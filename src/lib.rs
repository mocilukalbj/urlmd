use anyhow::{Context, Result, bail};
use dom_query::{Document, Matcher};
use encoding_rs::Encoding;
use url::Url;

mod links;
mod tables;
pub use links::{LinkMode, rewrite_links};

pub struct Extraction {
    pub markdown: String,
    pub title: String,
    pub selection: String,
}

fn charset_parameter(value: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|item| {
        let (key, val) = item.trim().split_once('=')?;
        key.trim().eq_ignore_ascii_case("charset").then(|| {
            val.trim().trim_matches(['\'', '"']).to_owned()
        })
    })
}

/// Respect explicit overrides, BOMs, HTTP charset, and early HTML meta declarations.
/// Reject decoding errors instead of silently saving mojibake.
pub fn decode(bytes: &[u8], content_type: &str, override_encoding: Option<&str>) -> Result<(String, String)> {
    let mut label = override_encoding.map(str::to_owned);
    let mut bom_skip = 0;
    if label.is_none() {
        if let Some((encoding, len)) = Encoding::for_bom(bytes) {
            label = Some(encoding.name().to_owned());
            bom_skip = len;
        } else {
            label = charset_parameter(content_type);
        }
    }
    let mime = content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    if label.is_none() && matches!(mime.as_str(), "text/html" | "application/xhtml+xml" | "") {
        // Encoding declarations live near the start; the actual conversion uses the whole response.
        let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
        let doc = Document::from(prefix.as_ref());
        if let Some(charset) = doc.select_single("meta[charset]").attr("charset") {
            label = Some(charset.to_string());
        } else {
            for meta in doc.select("meta[http-equiv][content]").iter() {
                if meta.attr("http-equiv").is_some_and(|v| v.eq_ignore_ascii_case("content-type")) {
                    label = meta.attr("content").and_then(|v| charset_parameter(&v));
                    if label.is_some() { break; }
                }
            }
        }
    }
    let label = label.unwrap_or_else(|| "utf-8".into());
    let encoding = Encoding::for_label(label.as_bytes())
        .with_context(|| format!("Unknown encoding {label:?}; use --encoding with a supported charset"))?;
    if override_encoding.is_some() && let Some((bom_encoding, len)) = Encoding::for_bom(bytes) {
        if bom_encoding == encoding { bom_skip = len; }
    }
    let (text, errors) = encoding.decode_without_bom_handling(&bytes[bom_skip..]);
    if errors {
        bail!("Invalid {} data; use --encoding to specify the page's charset", encoding.name());
    }
    Ok((text.into_owned(), encoding.name().to_owned()))
}

fn inline_hidden(style: &str) -> bool {
    style.split(';').any(|declaration| {
        let Some((property, value)) = declaration.split_once(':') else { return false; };
        let value: String = value.chars().filter(|c| !c.is_whitespace()).collect();
        let value = value.to_ascii_lowercase();
        let value = value.strip_suffix("!important").unwrap_or(&value);
        match property.trim().to_ascii_lowercase().as_str() {
            "display" => value == "none",
            "visibility" => matches!(value, "hidden" | "collapse"),
            _ => false,
        }
    })
}

fn collapse_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn valid_language(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '#'))
}

fn class_language(classes: &str) -> Option<String> {
    let words: Vec<_> = classes.split_whitespace().collect();
    words.iter().find_map(|word| word.strip_prefix("language-").map(str::to_owned))
        .or_else(|| words.windows(2).find(|pair| pair[0] == "brush:").map(|pair| pair[1].to_owned()))
        .or_else(|| words.contains(&"rust").then(|| "rust".to_owned()))
}

fn clean_document_controls(doc: &Document) {
    // Match known UI structures, not these words wherever they appear in prose/code.
    doc.select("button#copy-path,rustdoc-toolbar,.notable-trait-badge-container,a.test-arrow[href^='https://play.rust-lang.org/'],details.baseline-indicator,mdn-survey").remove();
    for anchor in doc.select("a.headerlink,a.doc-anchor,a.anchor,a.header-anchor,h1 a,h2 a,h3 a,h4 a,h5 a,h6 a").iter() {
        if matches!(anchor.text().trim(), "¶" | "§" | "#" | "\u{200b}" | "")
            && !anchor.select("img").exists()
            && anchor.attr("href").is_some_and(|href| href.starts_with('#')) {
            anchor.remove();
        }
    }
    for button in doc.select("button").iter() {
        if matches!(collapse_space(&button.text()).as_str(), "Copy pageCopy" | "Copy page") {
            button.remove();
        }
    }
    for summary in doc.select("details.top-doc > summary.hideme").iter() {
        if collapse_space(&summary.text()) == "Expand description" { summary.remove(); }
    }
    for example in doc.select(".code-example").iter() {
        let label = example.select_single(".example-header .language-name");
        let language = label.text();
        let language = language.trim();
        let pre = example.select_single("pre");
        if pre.exists() && valid_language(language) {
            if pre.attr("data-language").is_none() { pre.set_attr("data-language", language); }
            label.remove();
        }
        example.select(".example-header button").remove();
    }
}

pub fn html_to_markdown(html: &str, base_url: Option<&Url>, selector: Option<&str>, whole_page: bool) -> Result<Extraction> {
    let doc = Document::from(html);
    let title = collapse_space(&doc.select_single("title").text());
    // Keep alternate code tabs: a class named `hidden` alone is not an HTML visibility guarantee.
    doc.select("script,style,noscript,template,iframe,object,embed,svg,canvas,[hidden],input[type='hidden'],.syntax-highlighter-line-numbers").remove();
    for element in doc.select("[aria-hidden],[style]").iter() {
        if element.attr("aria-hidden").is_some_and(|v| v.eq_ignore_ascii_case("true"))
            || element.attr("style").is_some_and(|v| inline_hidden(&v)) {
            element.remove();
        }
    }
    if selector.is_none() && !whole_page {
        doc.select("nav,footer,[role='navigation'],#docContent > .navheader,#docContent > .navfooter").remove();
    }
    clean_document_controls(&doc);

    // Some authored documentation nests <pre> inside <code>. Do not serialize the
    // resulting fenced block as another inline code span.
    for code in doc.select("code").iter() {
        if code.select("pre").exists() { code.rename("div"); }
    }
    // Normalize common code-language attributes before the Markdown serializer sees them.
    for pre in doc.select("pre").iter() {
        for br in pre.select("br").iter() {
            br.before_html("\n");
            br.remove();
        }
        let original_code = pre.select_single("code");
        let lines = original_code.children().filter("div.line");
        // Twoslash uses block elements for lines, with no literal newline between them.
        // Keep diagnostics as prose outside the code rather than mixing UI into its text.
        let code_text = if lines.exists() {
            for diagnostic in original_code.select(".error").iter().rev() {
                pre.after_html(format!("<blockquote>{}</blockquote>", diagnostic.inner_html()));
            }
            let label = pre.select_single(".language-id").text();
            if valid_language(label.trim()) { pre.set_attr("data-language", label.trim()); }
            lines.iter().map(|line| line.text().to_string()).collect::<Vec<_>>().join("\n")
        } else { pre.text().to_string() };
        let language = original_code.attr("data-language").or_else(|| pre.attr("data-language"))
            .map(|value| value.trim().to_owned())
            .or_else(|| original_code.attr("class").and_then(|value| class_language(&value)))
            .or_else(|| pre.attr("class").and_then(|value| class_language(&value)));
        // Flatten highlighting without normalizing whitespace. Wrap bare <pre> contents too,
        // otherwise the serializer can emit prose instead of a fenced code block.
        pre.set_html("<code></code>");
        let code = pre.select_single("code");
        code.set_text(&code_text);
        if let Some(language) = language.filter(|value| valid_language(value)) {
            code.set_attr("class", &format!("language-{language}"));
        }
    }
    let effective_base = doc.base_uri().and_then(|value| {
        Url::parse(&value).ok().or_else(|| base_url.and_then(|base| base.join(&value).ok()))
    }).or_else(|| base_url.cloned());
    for image in doc.select("img").iter() {
        let src = image.attr("src").unwrap_or_default();
        if (src.is_empty() || src.to_ascii_lowercase().starts_with("data:"))
            && let Some(real) = image.attr("data-src") {
            image.set_attr("src", &real);
        }
    }
    for (selector, attr) in [("a[href]", "href"), ("img[src]", "src")] {
        for element in doc.select(selector).iter() {
            if let Some(value) = element.attr(attr) {
                let parsed = Url::parse(&value).ok().or_else(|| effective_base.as_ref().and_then(|base| base.join(&value).ok()));
                if let Some(url) = parsed {
                    let allowed = matches!(url.scheme(), "http" | "https")
                        || (attr == "href" && matches!(url.scheme(), "mailto" | "tel"));
                    if allowed { element.set_attr(attr, url.as_str()); }
                    else { element.remove_attr(attr); }
                }
            }
        }
    }

    let (fragment, selection) = if let Some(selector) = selector {
        let matcher = Matcher::new(selector).map_err(|e| anyhow::anyhow!("Invalid CSS selector {selector:?}: {e:?}"))?;
        let nodes = doc.select_matcher(&matcher);
        if !nodes.exists() { bail!("CSS selector {selector:?} matched no elements"); }
        (nodes.iter().map(|node| node.html().to_string()).collect::<Vec<_>>().join("\n"), selector.to_owned())
    } else if whole_page {
        (doc.select_single("body").inner_html().to_string(), "body (whole page)".into())
    } else {
        let mut selected = None;
        for candidate in ["#handbook-content", "#mw-content-text > .mw-parser-output", "#docContent", "body > .fancy", "article.markdown-body", ".theme-doc-markdown", ".article-body", "main article", "article", "main", "[role='main']", "body"] {
            let nodes = doc.select(candidate);
            // Multiple articles often indicate an index, so keep looking for its enclosing main.
            if nodes.length() == 1 && !nodes.text().trim().is_empty() {
                let mut html = nodes.html().to_string();
                if candidate == "#mw-content-text > .mw-parser-output" {
                    html = format!("{}\n{html}", doc.select_single("h1#firstHeading").html());
                }
                selected = Some((html, candidate.to_owned()));
                break;
            }
        }
        selected.context("No readable HTML body found; the page may require browser rendering")?
    };
    let markdown = tables::convert_fragment(&fragment).context("HTML to Markdown conversion failed")?;
    if markdown.trim().is_empty() { bail!("The page has no extractable content; it may require browser rendering"); }
    Ok(Extraction { markdown: format!("{}\n", markdown.trim()), title, selection })
}

pub fn native_markdown(text: &str) -> Extraction {
    let title = text.lines().find_map(|line| line.strip_prefix("# ")).unwrap_or_default().trim().to_owned();
    Extraction { markdown: format!("{}\n", text.trim()), title, selection: "native-markdown".into() }
}

pub fn is_html(text: &str) -> bool {
    let prefix = text.trim_start().chars().take(256).collect::<String>().to_ascii_lowercase();
    prefix.starts_with("<!doctype html") || prefix.starts_with("<html") || prefix.starts_with("<head") || prefix.starts_with("<body")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_gbk_from_html_meta() {
        let html = "<html><head><meta charset='gbk'></head><body>中文文档</body></html>";
        let (bytes, _, errors) = encoding_rs::GBK.encode(html);
        assert!(!errors);
        let (decoded, encoding) = decode(&bytes, "text/html", None).unwrap();
        assert_eq!(decoded, html);
        assert_eq!(encoding, "GBK");
    }

    #[test]
    fn explicit_encoding_does_not_get_replaced_by_bom() {
        let bytes = [0xff, 0xfe, b'A', 0];
        let (decoded, encoding) = decode(&bytes, "text/html", None).unwrap();
        assert_eq!(decoded, "A");
        assert_eq!(encoding, "UTF-16LE");
        assert!(decode(&bytes, "text/html", Some("utf-8")).is_err());
    }

    #[test]
    fn markdown_code_examples_do_not_change_charset() {
        let text = "# 中文文档\n\n```html\n<meta charset='gbk'>\n```\n";
        for mime in ["text/markdown", "text/x-markdown", "text/plain"] {
            let (decoded, encoding) = decode(text.as_bytes(), mime, None).unwrap();
            assert_eq!(decoded, text);
            assert_eq!(encoding, "UTF-8");
        }
    }

    #[test]
    fn highlighted_code_preserves_newlines_blank_lines_and_indentation() {
        let html = "<main><pre><code data-language='python'><span aria-hidden='true'>1\n2\n3</span><span class='syntax-highlighter-line'><span>from openai import OpenAI</span>\n</span><span class='syntax-highlighter-line'>\n</span><span class='syntax-highlighter-line'><span>if True:</span>\n</span><span class='syntax-highlighter-line'><span>    client = OpenAI()</span></span></code></pre></main>";
        let md = html_to_markdown(html, None, None, false).unwrap().markdown;
        assert!(md.contains("```python\nfrom openai import OpenAI\n\nif True:\n    client = OpenAI()\n```"), "{md}");
        assert!(!md.contains("1\n2\n3"));
    }

    #[test]
    fn removes_document_controls_but_preserves_heading_and_details_contents() {
        let html = "<main><h1 id='intro'>Guide<a class='headerlink' href='#intro'>¶</a><button id='copy-path'>Copy item path</button></h1>\
            <h2><a class='doc-anchor' href='#examples'>§</a>Examples</h2>\
            <h2>Implementations<a class='anchor' href='#impls'>§</a></h2>\
            <h2><a class='heading-anchor' href='#syntax'>Syntax</a></h2>\
            <details class='top-doc'><summary class='hideme'><span>Expand description</span></summary><p>Keep expanded body.</p></details>\
            <details><summary>Meaningful explanation</summary><p>Keep normal details.</p></details>\
            <a class='test-arrow' href='https://play.rust-lang.org/?code=long'><svg></svg></a>\
            <a href='/image'><img src='/diagram.png' alt='Diagram'></a>\
            <p>Explain Copy item path and ¶ in prose.</p><pre><code>Expand description\n§ http</code></pre></main>";
        let base = Url::parse("https://example.com/guide").unwrap();
        let md = html_to_markdown(html, Some(&base), None, false).unwrap().markdown;
        assert!(md.contains("# Guide\n"), "{md}");
        assert!(md.contains("## Examples\n"));
        assert!(md.contains("## Implementations\n"));
        assert!(md.contains("[Syntax](https://example.com/guide#syntax)"));
        assert!(md.contains("Keep expanded body."));
        assert!(md.contains("Meaningful explanation"));
        assert!(md.contains("Keep normal details."));
        assert!(!md.contains("play.rust-lang.org"));
        assert!(md.contains("[![Diagram](https://example.com/diagram.png)](https://example.com/image)"));
        assert!(md.contains("Explain Copy item path and ¶ in prose."));
        assert!(md.contains("```\nExpand description\n§ http\n```"));
    }

    #[test]
    fn mdn_language_labels_become_fence_languages_and_bare_pre_is_preserved() {
        let html = "<main><p>The protocol is http.</p><div class='code-example'><div class='example-header'><span class='language-name'>http</span></div>\
            <pre class='brush: http notranslate'><code>GET / HTTP/1.1\n\nHost: example.com\n</code></pre></div>\
            <pre class='brush: plain notranslate'>Content-Type: &lt;media-type&gt;\n</pre>\
            <pre class='rust rust-example-rendered'><code>fn main() {}\n</code></pre></main>";
        let md = html_to_markdown(html, None, None, false).unwrap().markdown;
        assert!(md.contains("```http\nGET / HTTP/1.1\n\nHost: example.com\n```"), "{md}");
        assert!(!md.contains("\nhttp\n"));
        assert!(md.contains("```plain\nContent-Type: <media-type>\n```"));
        assert!(md.contains("```rust\nfn main() {}\n```"));
        assert!(md.contains("The protocol is http."));
    }

    #[test]
    fn removes_badges_without_removing_traits_or_compatibility_content() {
        let html = "<main><h1>Vec</h1><div class='notable-trait-badge-container'><a href='/Write'>Write</a></div>\
            <details class='baseline-indicator high'><summary>Baseline Widely available</summary><p>This feature is well established</p></details>\
            <mdn-survey>Survey UI</mdn-survey><h2>Implementations</h2><h3>impl Write for Vec</h3>\
            <h2 id='browser_compatibility'>Browser compatibility</h2><p>Supported since version 10.</p>\
            <p>Baseline is a concept in this explanation.</p></main>";
        let md = html_to_markdown(html, None, None, false).unwrap().markdown;
        assert!(!md.contains("Widely available") && !md.contains("well established"));
        assert!(!md.contains("Survey UI"));
        assert!(!md.contains("[Write]"));
        assert!(md.contains("impl Write for Vec"));
        assert!(md.contains("Browser compatibility"));
        assert!(md.contains("Supported since version 10."));
        assert!(md.contains("Baseline is a concept in this explanation."));
    }

    #[test]
    fn twoslash_lines_preserve_code_and_keep_diagnostics_outside_fence() {
        let html = "<main><div id='handbook-content'><h1>Everyday Types</h1><article>\
            <pre class='shiki'><div class='language-id'>ts</div><div class='code-container'><code>\
            <div class='line'><span>function f() {</span></div><div class='line'>  return 42;</div>\
            <div class='line'></div><div class='line'>}</div><span class='error'>Type mismatch</span>\
            <span class='error-behind'>Type mismatch</span></code><a class='playground-link'>Try</a></div></pre>\
            </article></div></main>";
        let md = html_to_markdown(html, None, None, false).unwrap().markdown;
        assert!(md.starts_with("# Everyday Types\n"));
        assert!(md.contains("```ts\nfunction f() {\n  return 42;\n\n}\n```"), "{md}");
        assert!(md.contains("> Type mismatch"));
        assert_eq!(md.matches("Type mismatch").count(), 1);
        assert!(!md.contains("Try"));
    }

    #[test]
    fn known_document_containers_keep_titles_and_exclude_site_menus() {
        for (html, title, body) in [
            ("<main><h1 id='firstHeading'>HTTP</h1><div>Language menu</div><div id='mw-content-text'><div class='mw-parser-output'><p>Protocol body</p></div></div></main>", "HTTP", "Protocol body"),
            ("<div>Language menu</div><div id='docContent'><div class='navheader'>Navigation table</div><h2>Numeric types</h2><p>smallint</p><div class='navfooter'>Next page</div></div>", "Numeric types", "smallint"),
            ("<div>Language menu</div><div class='fancy'><h1>CREATE TABLE</h1><p>PRIMARY KEY</p></div>", "CREATE TABLE", "PRIMARY KEY"),
        ] {
            let md = html_to_markdown(html, None, None, false).unwrap().markdown;
            assert!(md.contains(title) && md.contains(body), "{md}");
            assert!(!md.contains("Language menu") && !md.contains("Navigation table") && !md.contains("Next page"));
            let whole = html_to_markdown(html, None, None, true).unwrap().markdown;
            assert!(whole.contains("Language menu"));
        }
    }
}
