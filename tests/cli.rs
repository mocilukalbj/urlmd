use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "urlmd-cli-test-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Response {
    status: &'static str,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    body: String,
    content_length: bool,
}

impl Response {
    fn html(body: impl Into<String>) -> Self {
        Self::text("text/html; charset=utf-8", body)
    }

    fn markdown(body: impl Into<String>) -> Self {
        Self::text("text/markdown; charset=utf-8", body)
    }

    fn text(content_type: &'static str, body: impl Into<String>) -> Self {
        Self {
            status: "200 OK",
            content_type,
            headers: Vec::new(),
            body: body.into(),
            content_length: true,
        }
    }

    fn redirect(location: &str) -> Self {
        let mut response = Self::html("");
        response.status = "302 Found";
        response.headers.push(("Location".into(), location.into()));
        response
    }
}

struct HttpFixture {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HttpFixture {
    fn new(handler: impl Fn(&str, &str) -> Response + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .expect("these HTTP integration tests need an available loopback listener");
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if let Some(request) = read_request(&mut stream) {
                            let path = request
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("/");
                            let response = handler(path, &request);
                            thread_requests.lock().unwrap().push(request);
                            write_response(&mut stream, response);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture listener failed: {error}"),
                }
            }
        });
        Self {
            base_url,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // Avoid obscuring a failed test assertion with a second panic.
            let _ = thread.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    while bytes.len() < 64 * 1024 {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).ok();
        }
    }
    None
}

fn write_response(stream: &mut TcpStream, response: Response) {
    let mut headers = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nConnection: close\r\n",
        response.status, response.content_type
    );
    if response.content_length {
        headers.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    }
    for (name, value) in response.headers {
        headers.push_str(&format!("{name}: {value}\r\n"));
    }
    headers.push_str("\r\n");
    let _ = stream.write_all(headers.as_bytes());
    let _ = stream.write_all(response.body.as_bytes());
    let _ = stream.flush();
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_urlmd"))
        .args(args)
        // Keep machine-level proxy configuration out of loopback fixtures.
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .expect("could not execute urlmd")
}

fn run_with_stdin(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_urlmd"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("could not execute urlmd with piped stdin");
    let mut stdin = child.stdin.take().expect("stdin pipe should be available");
    stdin.write_all(input).expect("could not write piped HTML");
    // Close the upstream pipe so urlmd can finish reading, as curl would do.
    drop(stdin);
    child.wait_with_output().expect("could not collect urlmd output")
}

fn succeeded(output: &Output) -> String {
    assert!(
        output.status.success(),
        "urlmd failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).expect("Markdown should be UTF-8")
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

#[test]
fn negotiates_native_markdown_and_preserves_it() {
    let native = "# Native document\n\nKeep **formatting** and [a link](https://example.com/).\n";
    let server = HttpFixture::new(move |_, request| {
        if header(request, "Accept")
            .unwrap_or("")
            .contains("text/markdown")
        {
            Response::markdown(native)
        } else {
            Response::html("<main><h1>Unexpected HTML fallback</h1></main>")
        }
    });
    let output = run(&[&server.url("/docs"), "--no-metadata"]);
    assert_eq!(succeeded(&output).trim(), native.trim());
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let accept = header(&requests[0], "Accept").expect("missing Accept header");
    let markdown_pos = accept.find("text/markdown").unwrap();
    if let Some(html_pos) = accept.find("text/html") {
        assert!(markdown_pos < html_pos, "Markdown should be preferred: {accept}");
    }
}

#[test]
fn sends_default_user_agent_and_explicit_override() {
    let server = HttpFixture::new(|_, _| Response::markdown("# User-Agent fixture\n"));
    let url = server.url("/agent");
    let output = run(&[&url, "--no-metadata"]);
    assert_eq!(succeeded(&output).trim(), "# User-Agent fixture");

    let custom = "Mozilla/5.0 (urlmd integration test)";
    let output = run(&[&url, "--user-agent", custom, "--no-metadata"]);
    assert_eq!(succeeded(&output).trim(), "# User-Agent fixture");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        header(&requests[0], "User-Agent"),
        Some(concat!("urlmd/", env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(header(&requests[1], "User-Agent"), Some(custom));
}

#[test]
fn invalid_user_agent_fails_before_request_without_overwriting_output() {
    let server = HttpFixture::new(|_, _| Response::markdown("# Should not be fetched\n"));
    let temp = TempDir::new();
    let destination = temp.path("existing.md");
    fs::write(&destination, "existing document\n").unwrap();
    for invalid in ["", "valid-agent\r\nX-Injected-Header: unexpected"] {
        let output = run(&[
            &server.url("/agent"),
            "--user-agent",
            invalid,
            "-o",
            path_arg(&destination),
        ]);
        assert!(!output.status.success(), "invalid User-Agent should fail");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty(), "the rejected value should be explained");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "existing document\n");
    }
    assert!(server.requests().is_empty(), "invalid header should fail before fetching");
}

#[test]
fn converts_complete_html_after_a_long_script_and_extracts_body() {
    let script = "x".repeat(160_000);
    let html = format!(
        "<!doctype html><html><head><title>Long page</title><script>{script}</script></head>\
         <body><nav>UNWANTED NAVIGATION</nav><main><h1>Document heading</h1>\
         <p>First useful paragraph.</p><pre><code class=\"language-rust\">fn main() {{ println!(\"hello\"); }}</code></pre>\
         <table><thead><tr><th>Setting</th><th>Value</th></tr></thead>\
         <tbody><tr><td>mode</td><td>precise</td></tr></tbody></table>\
         <p>BODY TAIL SENTINEL</p></main><footer>UNWANTED FOOTER</footer></body></html>"
    );
    let server = HttpFixture::new(move |_, _| Response::html(html.clone()));
    let output = run(&[&server.url("/long"), "--no-metadata"]);
    let md = succeeded(&output);
    for feature in [
        "Document heading",
        "First useful paragraph",
        "fn main()",
        "Setting",
        "Value",
        "precise",
        "BODY TAIL SENTINEL",
    ] {
        assert!(md.contains(feature), "lost feature {feature:?}: {md}");
    }
    assert!(md.contains("```"), "code block should remain fenced: {md}");
    assert!(!md.contains("UNWANTED NAVIGATION"));
    assert!(!md.contains("UNWANTED FOOTER"));
    assert!(!md.contains(&"x".repeat(100)), "script leaked into output");
}

#[test]
fn resolves_links_and_images_against_final_redirect_url() {
    let server = HttpFixture::new(|path, _| match path {
        "/start" => Response::redirect("/docs/guide/index.html"),
        "/docs/guide/index.html" => Response::html(
            "<main><h1>Redirect destination</h1><p><a href=\"../next.html\">Next chapter</a></p>\
             <img src=\"./diagram.png\" alt=\"Diagram\"></main>",
        ),
        _ => panic!("unexpected request path {path}"),
    });
    let output = run(&[&server.url("/start"), "--no-metadata"]);
    let md = succeeded(&output);
    assert!(md.contains("Redirect destination"));
    assert!(md.contains(&server.url("/docs/next.html")), "wrong link base: {md}");
    assert!(
        md.contains(&server.url("/docs/guide/diagram.png")),
        "wrong image base: {md}"
    );
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn forced_html_writes_markdown_and_preserves_source() {
    let html = "<!doctype html><main><h1>Forced HTML</h1><p>Saved source bytes.</p></main>";
    let server = HttpFixture::new(move |_, request| {
        let accept = header(request, "Accept").unwrap_or("");
        if accept.contains("text/markdown") && !accept.starts_with("text/html") {
            Response::markdown("# Negotiated Markdown instead\n")
        } else {
            Response::html(html)
        }
    });
    let temp = TempDir::new();
    let markdown_path = temp.path("document.md");
    let source_path = temp.path("document.html");
    let output = run(&[
        &server.url("/docs"),
        "--html",
        "--no-metadata",
        "-o",
        path_arg(&markdown_path),
        "--save-source",
        path_arg(&source_path),
    ]);
    assert!(succeeded(&output).is_empty(), "-o should not also print Markdown");
    assert!(fs::read_to_string(markdown_path).unwrap().contains("Forced HTML"));
    assert_eq!(fs::read(source_path).unwrap(), html.as_bytes());
    let requests = server.requests();
    let accept = header(&requests[0], "Accept").expect("missing Accept header");
    assert!(accept.contains("text/html"), "HTML request expected: {accept}");
    if let Some(markdown_pos) = accept.find("text/markdown") {
        assert!(accept.find("text/html").unwrap() < markdown_pos);
    }
}

#[test]
fn http_error_does_not_overwrite_existing_output() {
    let server = HttpFixture::new(|_, _| {
        let mut response = Response::html("<h1>Not found</h1>");
        response.status = "404 Not Found";
        response
    });
    let temp = TempDir::new();
    let destination = temp.path("existing.md");
    fs::write(&destination, "existing document\n").unwrap();
    let output = run(&[&server.url("/missing"), "-o", path_arg(&destination)]);
    assert!(!output.status.success(), "404 should fail");
    assert!(output.stdout.is_empty(), "an error page should not become Markdown");
    assert_eq!(fs::read_to_string(destination).unwrap(), "existing document\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("404"));
}

#[test]
fn rejects_oversized_bodies_with_and_without_content_length() {
    let server = HttpFixture::new(|path, _| {
        let mut response = Response::html(format!("<main><p>{}</p></main>", "x".repeat(2048)));
        response.content_length = path != "/without-length";
        response
    });
    let temp = TempDir::new();
    for (index, path) in ["/with-length", "/without-length"].iter().enumerate() {
        let destination = temp.path(&format!("too-large-{index}.md"));
        let output = run(&[
            &server.url(path),
            "--max-bytes",
            "128",
            "-o",
            path_arg(&destination),
        ]);
        assert!(!output.status.success(), "oversized response {path} should fail");
        assert!(output.stdout.is_empty());
        assert!(!destination.exists(), "partial Markdown should not be saved");
    }
}

#[test]
fn selector_combines_matches_and_rejects_invalid_or_empty_selection() {
    let html = "<body><nav>Other navigation</nav><section class=\"keep\"><h2>First section</h2></section>\
                <section class=\"keep\"><p>Second section</p></section></body>";
    let server = HttpFixture::new(move |_, _| Response::html(html));
    let url = server.url("/selection");
    let output = run(&[&url, "--selector", ".keep", "--no-metadata"]);
    let md = succeeded(&output);
    assert!(md.contains("First section"));
    assert!(md.contains("Second section"));
    assert!(!md.contains("Other navigation"));

    let temp = TempDir::new();
    for (index, selector) in ["[", ".missing"].iter().enumerate() {
        let destination = temp.path(&format!("failed-selector-{index}.md"));
        let source_path = temp.path(&format!("failed-selector-{index}.html"));
        let sidecar_path = temp.path(&format!("failed-selector-{index}.html.meta.json"));
        let output = run(&[
            &url,
            "--selector",
            selector,
            "-o",
            path_arg(&destination),
            "--save-source",
            path_arg(&source_path),
        ]);
        assert!(!output.status.success(), "selector {selector:?} should fail");
        assert!(output.stdout.is_empty());
        assert!(!destination.exists());
        assert_eq!(
            fs::read(source_path).unwrap(),
            html.as_bytes(),
            "downloaded source should remain available after extraction fails"
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(sidecar_path).unwrap()).unwrap();
        assert_eq!(metadata["extraction_status"], "failed");
    }
}

#[test]
fn offline_html_uses_explicit_base_url_without_network() {
    let temp = TempDir::new();
    let source = temp.path("saved.html");
    fs::write(
        &source,
        "<main><h1>Offline page</h1><a href=\"../next\">Next</a>\
         <img src=\"diagram.svg\" alt=\"Offline diagram\"></main>",
    )
    .unwrap();
    let output = run(&[
        "--input",
        path_arg(&source),
        "--base-url",
        "https://example.com/docs/current/",
        "--no-metadata",
    ]);
    let md = succeeded(&output);
    assert!(md.contains("Offline page"));
    assert!(md.contains("https://example.com/docs/next"), "wrong link base: {md}");
    assert!(
        md.contains("https://example.com/docs/current/diagram.svg"),
        "wrong image base: {md}"
    );
}

#[test]
fn piped_html_preserves_code_and_resolves_relative_urls_without_stdout_logs() {
    let html = "<!doctype html><html><body><nav>Unwanted pipeline navigation</nav>\
                <main><h1>Piped document</h1><p>Readable body from stdin.</p>\
                <pre><code class=\"language-python\">from example import Client\n\n\
if True:\n    client = Client()\n</code></pre>\
                <a href=\"../next?lang=zh#usage\">Next</a>\
                <img src=\"./images/diagram.svg\" alt=\"Pipeline diagram\"></main></body></html>";
    let output = run_with_stdin(
        &[
            "--input", "-", "--base-url", "https://example.com/docs/current/", "--no-metadata",
        ],
        html.as_bytes(),
    );
    let md = succeeded(&output);
    assert!(md.starts_with("# Piped document\n"), "stdout should start with Markdown: {md}");
    assert!(md.contains("Readable body from stdin."));
    assert!(
        md.contains("from example import Client\n\nif True:\n    client = Client()"),
        "piped code lost a newline, blank line or indentation: {md}"
    );
    assert!(md.contains("```python"), "code language should be preserved: {md}");
    assert!(md.contains("https://example.com/docs/next?lang=zh#usage"), "wrong link base: {md}");
    assert!(
        md.contains("https://example.com/docs/current/images/diagram.svg"),
        "wrong image base: {md}"
    );
    assert!(!md.contains("Unwanted pipeline navigation"));
    assert!(output.stderr.is_empty(), "successful conversion should be quiet");
}

#[test]
fn oversized_stdin_fails_without_overwriting_existing_output() {
    let temp = TempDir::new();
    let destination = temp.path("existing.md");
    fs::write(&destination, "existing document\n").unwrap();
    let html = format!("<main><h1>Too large</h1><p>{}</p></main>", "x".repeat(2048));
    let output = run_with_stdin(
        &["--input", "-", "--max-bytes", "128", "-o", path_arg(&destination)],
        html.as_bytes(),
    );
    assert!(!output.status.success(), "oversized stdin should fail");
    assert!(output.stdout.is_empty(), "partial Markdown should not be printed");
    assert_eq!(fs::read_to_string(destination).unwrap(), "existing document\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("--max-bytes 128"));
}

#[test]
fn empty_stdin_fails_without_overwriting_existing_output() {
    let temp = TempDir::new();
    let destination = temp.path("existing.md");
    fs::write(&destination, "existing document\n").unwrap();
    let output = run_with_stdin(&["--input", "-", "-o", path_arg(&destination)], b"");
    assert!(!output.status.success(), "empty stdin should fail");
    assert!(output.stdout.is_empty(), "empty input should not produce Markdown");
    assert_eq!(fs::read_to_string(destination).unwrap(), "existing document\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("HTML body"));
}

#[test]
fn output_cannot_collide_with_source_or_its_metadata_sidecar() {
    let temp = TempDir::new();
    let input = temp.path("input.html");
    fs::write(&input, "<main><h1>Input document</h1></main>").unwrap();
    fs::create_dir(temp.path("sub")).unwrap();
    let source = temp.path("page.html");
    let sidecar = temp.path("page.html.meta.json");
    let alias = temp.path("sub/../page.html");

    for destination in [&sidecar, &alias] {
        let output = run(&[
            "--input",
            path_arg(&input),
            "--save-source",
            path_arg(&source),
            "-o",
            path_arg(destination),
        ]);
        assert!(
            !output.status.success(),
            "conflicting output {} should fail",
            destination.display()
        );
        assert!(output.stdout.is_empty());
        assert!(!source.exists(), "conflict must be checked before saving source");
        assert!(!sidecar.exists(), "conflict must be checked before saving metadata");
        assert!(!destination.exists(), "conflict must not create output");
    }
    assert_eq!(
        fs::read_to_string(input).unwrap(),
        "<main><h1>Input document</h1></main>"
    );
}

#[test]
fn offline_html_resolves_absolute_base_element_without_base_url_argument() {
    let temp = TempDir::new();
    let source = temp.path("with-base.html");
    fs::write(
        &source,
        "<!doctype html><html><head><base href=\"https://example.com/docs/current/\"></head>\
         <body><main><h1>HTML base document</h1><a href=\"../next?lang=zh#usage\">Next</a>\
         <img src=\"diagram.svg\" alt=\"Base diagram\"></main></body></html>",
    )
    .unwrap();
    let output = run(&["--input", path_arg(&source), "--no-metadata"]);
    let md = succeeded(&output);
    assert!(md.contains("HTML base document"));
    assert!(
        md.contains("https://example.com/docs/next?lang=zh#usage"),
        "absolute HTML base should resolve relative links: {md}"
    );
    assert!(
        md.contains("https://example.com/docs/current/diagram.svg"),
        "absolute HTML base should resolve relative images: {md}"
    );
}
