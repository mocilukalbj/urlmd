use std::{fs, io::{self, Read, Write}, path::{Component, Path, PathBuf}, time::Duration};
use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use clap::Parser;
use reqwest::{blocking::Client, header::{ACCEPT, CONTENT_TYPE, HeaderValue}};
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;
use urlmd::{LinkMode, decode, html_to_markdown, is_html, native_markdown, rewrite_links};

/// Fetch a URL as Markdown. Prefer native Markdown; otherwise extract HTML content.
#[derive(Parser, Debug)]
#[command(version, about, after_help = "Examples:\n  urlmd https://developers.openai.com/api/docs -o docs.md\n  urlmd https://example.com/guide --html --save-source page.html -o page.md\n  urlmd --input page.html --base-url https://example.com/guide --selector main")]
struct Args {
    /// HTTP(S) URL. With --input, supply --base-url instead.
    #[arg(required_unless_present = "input", conflicts_with = "input")]
    url: Option<String>,
    /// Read saved HTML or a .md file instead of downloading (- reads HTML from stdin).
    #[arg(long, value_name = "FILE")]
    input: Option<PathBuf>,
    /// Resolve relative links in saved HTML against this URL.
    #[arg(long, requires = "input", value_name = "URL")]
    base_url: Option<String>,
    /// Write Markdown to a file; default is stdout (-).
    #[arg(short, long, default_value = "-", value_name = "FILE")]
    output: PathBuf,
    /// Request HTML even when the server offers native Markdown.
    #[arg(long)]
    html: bool,
    /// Extract all matching CSS elements (implies an HTML request).
    #[arg(long, conflicts_with = "whole_page", value_name = "CSS")]
    selector: Option<String>,
    /// Convert the complete body, including navigation (implies an HTML request).
    #[arg(long)]
    whole_page: bool,
    /// Save original response bytes and a FILE.meta.json provenance sidecar.
    #[arg(long, value_name = "FILE")]
    save_source: Option<PathBuf>,
    /// Omit YAML provenance front matter from the Markdown.
    #[arg(long)]
    no_metadata: bool,
    /// Link destinations: keep, shorten same-origin URLs, or keep only labels.
    #[arg(long, value_enum, default_value_t = LinkMode::Keep)]
    links: LinkMode,
    /// Override the source charset, e.g. utf-8 or gbk.
    #[arg(long, value_name = "CHARSET")]
    encoding: Option<String>,
    /// Maximum decoded HTTP response bytes; over-limit inputs fail rather than truncate.
    #[arg(long, default_value_t = 20_971_520, value_parser = clap::value_parser!(u64).range(1..=1_073_741_824))]
    max_bytes: u64,
    /// Network timeout in seconds.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout: u64,
    /// User-Agent for URL requests; ignored when reading --input.
    #[arg(long, value_name = "UA", default_value = concat!("urlmd/", env!("CARGO_PKG_VERSION")), value_parser = parse_user_agent)]
    user_agent: String,
}

fn parse_user_agent(value: &str) -> std::result::Result<String, String> {
    if value.trim().is_empty() { return Err("User-Agent must not be empty".into()); }
    value.parse::<HeaderValue>().map_err(|_| "User-Agent must be a valid HTTP header value".to_owned())?;
    Ok(value.to_owned())
}

#[derive(Serialize)]
struct Metadata {
    source: String,
    final_url: Option<String>,
    title: String,
    fetched_at: Option<String>,
    converted_at: Option<String>,
    content_type: String,
    encoding: String,
    source_bytes: usize,
    source_sha256: String,
    format: String,
    selection: String,
    links: &'static str,
    extractor: String,
    extraction_status: String,
    error: Option<String>,
}

fn http_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).with_context(|| format!("Invalid URL {value:?}"))?;
    if !matches!(url.scheme(), "http" | "https") { bail!("Only HTTP(S) URLs are supported"); }
    if !url.username().is_empty() || url.password().is_some() { bail!("URLs containing credentials are not supported"); }
    Ok(url)
}

fn read_bounded(reader: impl Read, max_bytes: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(max_bytes + 1).read_to_end(&mut bytes).context("Failed to read source")?;
    if bytes.len() as u64 > max_bytes { bail!("Source exceeds --max-bytes {max_bytes}; no truncated Markdown was produced"); }
    Ok(bytes)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("Cannot write {}", path.display()))
}

fn sidecar_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".meta.json");
    PathBuf::from(value)
}

fn normalized_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() { path.to_owned() } else { std::env::current_dir()?.join(path) };
    // Canonicalize the nearest existing ancestor, including directory symlinks.
    let mut ancestor = absolute;
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(ancestor.components().next_back().context("Cannot normalize output path")?.as_os_str().to_os_string());
        if !ancestor.pop() { bail!("Cannot normalize {}", path.display()); }
    }
    let mut result = fs::canonicalize(ancestor)?;
    for component in suffix.into_iter().rev() {
        match Path::new(&component).components().next() {
            Some(Component::CurDir) => {},
            Some(Component::ParentDir) => { result.pop(); },
            _ => result.push(component),
        }
    }
    Ok(result)
}

fn save_metadata(path: &Path, metadata: &Metadata) -> Result<()> {
    let mut json = serde_json::to_vec_pretty(metadata)?;
    json.push(b'\n');
    write_file(&sidecar_path(path), &json)
}

fn run(args: Args) -> Result<()> {
    if let Some(source) = &args.save_source {
        let source_path = normalized_path(source)?;
        let sidecar = normalized_path(&sidecar_path(source))?;
        if source_path == sidecar { bail!("--save-source and its .meta.json sidecar must use different files"); }
        if args.output != Path::new("-") {
            let output = normalized_path(&args.output)?;
            if output == source_path || output == sidecar {
                bail!("--output must differ from --save-source and its .meta.json sidecar");
            }
        }
    }
    let force_html = args.html || args.selector.is_some() || args.whole_page;
    let (source, final_url, content_type, bytes) = if let Some(input) = &args.input {
        let bytes = if input == Path::new("-") { read_bounded(io::stdin().lock(), args.max_bytes)? }
            else { read_bounded(fs::File::open(input).with_context(|| format!("Cannot open {}", input.display()))?, args.max_bytes)? };
        let is_md = input.extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown"));
        (input.display().to_string(), args.base_url.as_deref().map(http_url).transpose()?, if is_md { "text/markdown" } else { "text/html" }.to_owned(), bytes)
    } else {
        let source = args.url.as_ref().unwrap().clone();
        let url = http_url(&source)?;
        let client = Client::builder().timeout(Duration::from_secs(args.timeout))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent(&args.user_agent)
            .build().context("Cannot initialize HTTP client")?;
        let accept = if force_html { "text/html, application/xhtml+xml;q=0.9" }
            else { "text/markdown, text/html;q=0.9, text/plain;q=0.8, */*;q=0.1" };
        let response = client.get(url).header(ACCEPT, accept).send()
            .context("Request failed")?.error_for_status().context("HTTP request failed")?;
        let final_url = response.url().clone();
        let content_type = response.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
        if response.content_length().is_some_and(|length| length > args.max_bytes) {
            bail!("Source exceeds --max-bytes {}; no truncated Markdown was produced", args.max_bytes);
        }
        let bytes = read_bounded(response, args.max_bytes)?;
        (source, Some(final_url), content_type, bytes)
    };
    let mut metadata = Metadata {
        source, final_url: final_url.as_ref().map(|u| u.to_string()), title: String::new(),
        fetched_at: args.input.is_none().then(|| Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
        converted_at: None, content_type, encoding: String::new(),
        source_bytes: bytes.len(), source_sha256: format!("{:x}", Sha256::digest(&bytes)),
        format: "unprocessed".into(), selection: String::new(),
        links: args.links.as_str(),
        extractor: concat!("urlmd/", env!("CARGO_PKG_VERSION")).into(),
        extraction_status: "downloaded".into(), error: None,
    };
    if let Some(path) = &args.save_source {
        write_file(path, &bytes)?;
        save_metadata(path, &metadata)?;
    }
    let converted = (|| -> Result<_> {
        let (text, encoding) = decode(&bytes, &metadata.content_type, args.encoding.as_deref())?;
        let mime = metadata.content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        let native = !is_html(&text) && matches!(mime.as_str(), "text/markdown" | "text/x-markdown" | "text/plain");
        if force_html && native { bail!("The source returned Markdown or plain text despite the HTML request; remove --html/--selector/--whole-page"); }
        let mut extraction = if native { native_markdown(&text) }
            else if matches!(mime.as_str(), "text/html" | "application/xhtml+xml" | "") || is_html(&text) {
                html_to_markdown(&text, final_url.as_ref(), args.selector.as_deref(), args.whole_page)?
            } else { bail!("Unsupported Content-Type {:?}; expected HTML or Markdown", metadata.content_type); };
        if extraction.markdown.trim().is_empty() { bail!("Empty Markdown response"); }
        extraction.markdown = rewrite_links(&extraction.markdown, final_url.as_ref(), args.links);
        let format = if native && mime == "text/plain" { "plain-text" }
            else if native { "native-markdown" } else { "html" };
        Ok((extraction, encoding, format))
    })();
    let (extraction, encoding, format) = match converted {
        Ok(value) => value,
        Err(error) => {
            metadata.extraction_status = "failed".into();
            metadata.error = Some(format!("{error:#}"));
            if let Some(path) = &args.save_source { save_metadata(path, &metadata)?; }
            return Err(error);
        }
    };
    metadata.title = extraction.title;
    metadata.encoding = encoding;
    metadata.format = format.into();
    metadata.selection = extraction.selection;
    metadata.converted_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
    metadata.extraction_status = "converted".into();
    let output = if args.no_metadata { extraction.markdown }
        else {
            let quote = |s: &str| serde_json::to_string(s).unwrap();
            format!("---\nsource: {}\nurl: {}\ntitle: {}\nfetched_at: {}\nconverted_at: {}\nformat: {}\nselection: {}\nlinks: {}\nextractor: {}\n---\n\n{}",
                quote(&metadata.source), quote(metadata.final_url.as_deref().unwrap_or_default()), quote(&metadata.title),
                metadata.fetched_at.as_deref().map(quote).unwrap_or_else(|| "null".into()),
                quote(metadata.converted_at.as_deref().unwrap_or_default()), quote(&metadata.format), quote(&metadata.selection), quote(metadata.links), quote(&metadata.extractor), extraction.markdown)
        };
    if let Some(path) = &args.save_source {
        save_metadata(path, &metadata)?;
    }
    if args.output == Path::new("-") {
        match io::stdout().lock().write_all(output.as_bytes()) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            result => result.context("Cannot write Markdown to stdout")?,
        }
    } else {
        write_file(&args.output, output.as_bytes())?;
        eprintln!("Saved {} ({} bytes, {}, {})", args.output.display(), output.len(), metadata.format, metadata.selection);
    }
    Ok(())
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("urlmd: {error:#}");
        std::process::exit(1);
    }
}
