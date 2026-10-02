use std::{collections::HashSet, ops::Range};
use clap::ValueEnum;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use url::Url;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum LinkMode {
    /// Preserve links (the default).
    #[default]
    Keep,
    /// Shorten same-origin links relative to the source URL.
    Relative,
    /// Keep link labels without their destinations; preserve images and code.
    Text,
}

impl LinkMode {
    pub fn as_str(self) -> &'static str {
        match self { Self::Keep => "keep", Self::Relative => "relative", Self::Text => "text" }
    }
}

fn relative_target(target: &str, base: &Url) -> Option<String> {
    let url = Url::parse(target).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.origin() != base.origin()
        || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if url.path() == base.path() && url.query() == base.query()
        && let Some(fragment) = url.fragment() {
        return Some(format!("#{fragment}"));
    }
    // A path starting // would be interpreted as a different host.
    if url.path().starts_with("//") { return None; }
    Some(url[url::Position::BeforePath..].to_owned())
}

struct Link {
    span: Range<usize>,
    label: Option<Range<usize>>,
    replacement_target: Option<String>,
    title: String,
    definition: Option<(usize, usize)>,
}

/// Edit only parsed Markdown link spans. Preserve every other source byte, including
/// code, MDX/HTML, front matter and images. Reference definitions shared by images survive.
pub fn rewrite_links(markdown: &str, base: Option<&Url>, mode: LinkMode) -> String {
    if mode == LinkMode::Keep || (mode == LinkMode::Relative && base.is_none()) {
        return markdown.to_owned();
    }
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS;
    let mut parser = Parser::new_ext(markdown, options).into_offset_iter();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut removed_refs = HashSet::new();
    let mut retained_refs = HashSet::new();
    let mut active: Option<Link> = None;
    let mut image_depth = 0;
    while let Some((event, mut span)) = parser.next() {
        // pulldown-cmark's collapsed-reference span omits the trailing empty brackets.
        if matches!(&event, Event::Start(Tag::Link { link_type: LinkType::Collapsed, .. }
            | Tag::Image { link_type: LinkType::Collapsed, .. }))
            && markdown[span.end..].starts_with("[]") {
            span.end += 2;
        }
        let definition = match &event {
            Event::Start(Tag::Link { id, .. } | Tag::Image { id, .. }) if !id.is_empty() => {
                parser.reference_definitions().get(id).map(|d| (d.span.start, d.span.end))
            }
            _ => None,
        };
        if let Event::Start(Tag::Image { .. }) = &event {
            image_depth += 1;
            if let Some(definition) = definition { retained_refs.insert(definition); }
        }
        if matches!(&event, Event::End(TagEnd::Link)) && image_depth == 0 {
            if let Some(link) = active.take() {
                let label = link.label.map(|r| &markdown[r]).unwrap_or("");
                let replacement = if mode == LinkMode::Text { label.to_owned() }
                    else if let Some(target) = link.replacement_target {
                        let title = if link.title.is_empty() { String::new() }
                            else { format!(" \"{}\"", link.title.replace('\\', "\\\\").replace('"', "\\\"")) };
                        format!("[{label}](<{target}>{title})")
                    } else { continue; };
                edits.push((link.span, replacement));
                if let Some(definition) = link.definition { removed_refs.insert(definition); }
            }
        } else if let Event::Start(Tag::Link { dest_url, title, .. }) = &event {
            if image_depth == 0 {
                let replacement_target = base.and_then(|base| relative_target(dest_url, base));
                if mode == LinkMode::Text || replacement_target.is_some() {
                    active = Some(Link { span, label: None, replacement_target, title: title.to_string(), definition });
                } else if let Some(definition) = definition { retained_refs.insert(definition); }
            } else if let Some(definition) = definition { retained_refs.insert(definition); }
        } else if let Some(link) = &mut active {
            match &mut link.label {
                Some(label) => { label.start = label.start.min(span.start); label.end = label.end.max(span.end); }
                None => link.label = Some(span),
            }
        }
        if matches!(event, Event::End(TagEnd::Image)) { image_depth -= 1; }
    }
    for &(start, end) in removed_refs.difference(&retained_refs) {
        edits.push((start..end, String::new()));
    }
    edits.sort_by_key(|(span, _)| span.start);
    let mut output = String::with_capacity(markdown.len());
    let mut cursor = 0;
    for (span, replacement) in edits {
        // Never apply overlapping edits to malformed or nested source constructs.
        if span.start < cursor { continue; }
        output.push_str(&markdown[cursor..span.start]);
        output.push_str(&replacement);
        cursor = span.end;
    }
    output.push_str(&markdown[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_preserves_formatting_code_images_and_front_matter() {
        let md = "---\ntitle: '[metadata](https://keep.test)'\n---\n\n[**中文** `code`](https://example.com/a_(b) \"title\")\n\n`[literal](https://keep.test)`\n\n```md\n[example](https://keep.test)\n```\n\n[![diagram](https://img.test/a.png)](https://remove.test)\n";
        let output = rewrite_links(md, None, LinkMode::Text);
        assert!(output.contains("\n**中文** `code`\n"), "{output}");
        assert!(output.contains("title: '[metadata](https://keep.test)'"));
        assert!(output.contains("`[literal](https://keep.test)`"));
        assert!(output.contains("```md\n[example](https://keep.test)\n```"));
        assert!(output.contains("![diagram](https://img.test/a.png)"));
        assert!(!output.contains("https://remove.test"));
    }

    #[test]
    fn reference_links_drop_unused_definitions_but_keep_image_references() {
        let md = "[One][id], [id][], [id] and [Pic][image].\n\n![actual][image]\n\n[id]: https://example.com/long \"Title\"\n[image]: https://example.com/pic.png\n";
        let output = rewrite_links(md, None, LinkMode::Text);
        assert!(output.starts_with("One, id, id and Pic."), "{output}");
        assert!(!output.contains("https://example.com/long"));
        assert!(output.contains("![actual][image]"));
        assert!(output.contains("[image]: https://example.com/pic.png"));
    }

    #[test]
    fn relative_links_preserve_queries_external_origins_and_images() {
        let base = Url::parse("https://example.com/docs/page?q=1").unwrap();
        let md = "[here](https://example.com/docs/page?q=1#part) [next][n] [external](https://other.test/a) [http](http://example.com/a) ![img](https://example.com/i.png)\n\n[n]: https://example.com/next?q=2#end\n";
        let output = rewrite_links(md, Some(&base), LinkMode::Relative);
        assert!(output.contains("[here](<#part>)"), "{output}");
        assert!(output.contains("[next](</next?q=2#end>)"));
        assert!(output.contains("[external](https://other.test/a)"));
        assert!(output.contains("[http](http://example.com/a)"));
        assert!(output.contains("![img](https://example.com/i.png)"));
        assert!(!output.contains("[n]:"));
        assert!(relative_target("https://example.com//another.test/path", &base).is_none());
    }

    #[test]
    fn autolinks_and_literal_urls_keep_their_visible_text() {
        let md = "<https://example.com/a> <user@example.com> https://example.com/literal\n\n[](/empty) [escaped \\] label](/link)\n";
        let output = rewrite_links(md, None, LinkMode::Text);
        assert_eq!(output, "https://example.com/a user@example.com https://example.com/literal\n\n escaped \\] label\n");
        assert_eq!(rewrite_links(md, None, LinkMode::Keep), md);
    }
}
