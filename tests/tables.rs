use pulldown_cmark::{Event, Options, Parser, Tag};
use urlmd::{LinkMode, html_to_markdown, rewrite_links};
use url::Url;

fn convert(html: &str) -> String {
    html_to_markdown(html, Some(&Url::parse("https://example.com/docs/").unwrap()), None, false).unwrap().markdown
}

fn table_rows(md: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = None;
    for event in Parser::new_ext(md, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::TableHead | Tag::TableRow) => row.clear(),
            Event::Start(Tag::TableCell) => cell = Some(String::new()),
            Event::Text(text) | Event::Code(text) => { if let Some(cell) = &mut cell { cell.push_str(&text); } },
            Event::End(pulldown_cmark::TagEnd::TableCell) => row.push(cell.take().unwrap()),
            Event::End(pulldown_cmark::TagEnd::TableHead | pulldown_cmark::TagEnd::TableRow) => rows.push(row.clone()),
            _ => {},
        }
    }
    rows
}

#[test]
fn row_headers_preserve_cell_positions_with_and_without_thead() {
    for html in [
        "<table><tr><th>K</th><td>V1</td></tr><tr><th>K2</th><td>V2</td></tr></table>",
        "<table><tr><th scope='row'>K</th><td>V1</td></tr><tr><th scope='row'>K2</th><td>V2</td></tr></table>",
        "<table><thead><tr><th>Property</th><th>Value</th></tr></thead><tbody><tr><th scope='row'>K</th><td>V1</td></tr><tr><th>K2</th><td>V2</td></tr></tbody></table>",
    ] {
        let md = convert(html);
        let rows = table_rows(&md);
        assert_eq!(rows.len(), 3, "{md}");
        assert_eq!(rows[1], ["K", "V1"], "{md}");
        assert_eq!(rows[2], ["K2", "V2"], "{md}");
    }
}

#[test]
fn preserves_mixed_thead_cells_body_headers_caption_and_footer() {
    let md = convert("<table><caption>API properties</caption><thead><tr><th>Name</th><td>Value</td></tr></thead>\
        <tbody><tr><th>A</th><td>first</td></tr></tbody><tbody><tr><td>B</td><th>second</th></tr></tbody>\
        <tfoot><tr><th>Total</th><td>2</td></tr></tfoot></table>");
    assert!(md.contains("API properties"));
    assert_eq!(table_rows(&md), vec![vec!["Name", "Value"],vec!["A", "first"],vec!["B", "second"],vec!["Total", "2"]]);
}

#[test]
fn headerless_and_ragged_tables_keep_the_first_data_row() {
    let md = convert("<table><tr><td>A1</td><td>B1</td></tr><tr><td>A2</td><td>B2</td><td>C2</td></tr><tr><td>A3</td></tr></table>");
    let rows = table_rows(&md);
    assert_eq!(rows[1], ["A1", "B1", ""]);
    assert_eq!(rows[2], ["A2", "B2", "C2"]);
    assert_eq!(rows[3], ["A3", "", ""]);
}

#[test]
fn mdn_properties_links_and_inline_code_survive_all_link_modes() {
    let md = convert("<main><table class='properties'>\
        <tr><th scope='row'>Header type</th><td><a href='request'>Request header</a>, <a href='response'>Response header</a></td></tr>\
        <tr><th>Forbidden request header</th><td>No</td></tr>\
        <tr><th>CORS-safelisted request header</th><td>Yes*; <code>a|b</code></td></tr></table></main>");
    let base = Url::parse("https://example.com/docs/").unwrap();
    for mode in [LinkMode::Keep, LinkMode::Relative, LinkMode::Text] {
        let output = rewrite_links(&md, Some(&base), mode);
        let rows = table_rows(&output);
        assert_eq!(rows[1], ["Header type", "Request header, Response header"], "{output}");
        assert_eq!(rows[2], ["Forbidden request header", "No"]);
        assert!(rows[3][1].contains("a|b"), "{output}");
    }
}

#[test]
fn complex_tables_preserve_all_cells_spans_and_multiple_header_rows() {
    let md = convert("<table><caption>Grouped data</caption><thead>\
        <tr><th rowspan='2'>Name</th><th colspan='2'>Scores</th></tr><tr><th>Low</th><th>High</th></tr></thead>\
        <tbody><tr><th scope='row'>Alice</th><td>11</td><td>22</td></tr><tr><th>Bob</th><td colspan='2'>33</td></tr></tbody></table>");
    for value in ["Grouped data", "Name", "Scores", "Low", "High", "Alice", "11", "22", "Bob", "33", "rowspan=2", "colspan=2"] {
        assert!(md.contains(value), "missing {value}: {md}");
    }
}

#[test]
fn nested_tables_lists_and_multiline_code_keep_their_structure() {
    let md = convert("<table><tr><th>Outer</th><td><table><tr><th>Inner</th><td>Nested value</td></tr></table>\
        <ul><li>First item</li><li>Second item</li></ul><pre><code class='language-python'>if True:\n    run()\n\nfinish()</code></pre></td></tr></table>");
    for value in ["Outer", "Inner", "Nested value", "First item", "Second item"] { assert!(md.contains(value), "{md}"); }
    let mut blocks = Vec::new();
    let mut code = None;
    for event in Parser::new_ext(&md, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::Text(text) => { if let Some(code) = &mut code { code.push_str(&text); } },
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => blocks.push(code.take().unwrap()),
            _ => {},
        }
    }
    assert_eq!(blocks, ["if True:\n    run()\n\nfinish()\n"], "{md}");
}

#[test]
fn empty_cells_and_empty_headers_still_make_valid_markdown_tables() {
    let md = convert("<table><thead><tr><th></th><th>Value</th></tr></thead><tr><th></th><td>one</td></tr><tr><th></th><td></td></tr></table>");
    let rows = table_rows(&md);
    assert_eq!(rows.len(), 3, "{md}");
    assert_eq!(rows[1], ["", "one"]);
    assert_eq!(rows[2], ["", ""]);
}

#[test]
fn code_wrapping_pre_is_a_block_and_br_preserves_line_breaks() {
    let md = convert("<table><tr><td><p>Example</p><code><pre>let a = 1;<br>let b = 2;</pre></code></td></tr></table>");
    let mut code = String::new();
    let mut in_code = false;
    let mut count = 0;
    for event in Parser::new(&md) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => { in_code = true; count += 1; },
            Event::Text(text) if in_code => code.push_str(&text),
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => in_code = false,
            _ => {},
        }
    }
    assert_eq!(count, 1, "{md}");
    assert_eq!(code, "let a = 1;\nlet b = 2;\n", "{md}");
}

#[test]
fn internal_placeholders_cannot_be_supplied_by_input() {
    let md = convert("<urlmd-table data-index='0'>Keep input text</urlmd-table><table><tr><td>Real cell</td></tr></table>");
    assert!(md.contains("Keep input text"));
    assert_eq!(md.matches("Real cell").count(), 1);
}
