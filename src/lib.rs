use anyhow::{Context, Result, bail};
use dom_query::{Document, Matcher};
use encoding_rs::Encoding;
use htmd::{HtmlToMarkdown, options::{BulletListMarker, Options}};
use url::Url;

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
    if label.is_none() && !content_type.to_ascii_lowercase().starts_with("text/markdown") {
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
        doc.select("nav,footer,[role='navigation']").remove();
    }

    // Normalize common code-language attributes before the Markdown serializer sees them.
    for code in doc.select("pre code").iter() {
        // Highlighting spans can normalize whitespace in the serializer. Flatten them while
        // preserving raw textContent, including blank lines and indentation.
        let code_text = code.text();
        code.set_text(&code_text);
        if let Some(language) = code.attr("data-language").or_else(|| code.parent().attr("data-language")) {
            let language = language.trim();
            if !language.is_empty() && language.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '#')) {
                let original = code.attr("class").unwrap_or_default();
                let mut classes = original.split_whitespace().filter(|c| !c.starts_with("language-")).map(str::to_owned).collect::<Vec<_>>();
                classes.push(format!("language-{language}"));
                code.set_attr("class", &classes.join(" "));
            }
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
        for candidate in ["article.markdown-body", ".theme-doc-markdown", ".article-body", "main article", "article", "main", "[role='main']", "body"] {
            let nodes = doc.select(candidate);
            // Multiple articles often indicate an index, so keep looking for its enclosing main.
            if nodes.length() == 1 && !nodes.text().trim().is_empty() {
                selected = Some((nodes.html().to_string(), candidate.to_owned()));
                break;
            }
        }
        selected.context("No readable HTML body found; the page may require browser rendering")?
    };
    let converter = HtmlToMarkdown::builder()
        .options(Options { bullet_list_marker: BulletListMarker::Dash, ..Default::default() })
        .skip_tags(vec!["script", "style", "noscript", "template", "svg", "canvas"])
        .build();
    let markdown = converter.convert(&fragment).context("HTML to Markdown conversion failed")?;
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
        let (decoded, encoding) = decode(text.as_bytes(), "text/markdown", None).unwrap();
        assert_eq!(decoded, text);
        assert_eq!(encoding, "UTF-8");
    }

    #[test]
    fn highlighted_code_preserves_newlines_blank_lines_and_indentation() {
        let html = "<main><pre><code data-language='python'><span aria-hidden='true'>1\n2\n3</span><span class='syntax-highlighter-line'><span>from openai import OpenAI</span>\n</span><span class='syntax-highlighter-line'>\n</span><span class='syntax-highlighter-line'><span>if True:</span>\n</span><span class='syntax-highlighter-line'><span>    client = OpenAI()</span></span></code></pre></main>";
        let md = html_to_markdown(html, None, None, false).unwrap().markdown;
        assert!(md.contains("```python\nfrom openai import OpenAI\n\nif True:\n    client = OpenAI()\n```"), "{md}");
        assert!(!md.contains("1\n2\n3"));
    }
}
