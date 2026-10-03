use std::sync::{Arc, Mutex};
use anyhow::Result;
use dom_query::{Document, Selection};
use htmd::{HtmlToMarkdown, options::{BulletListMarker, Options}};

struct Cell {
    markdown: String,
    header: bool,
    row_header: bool,
    spans: Vec<String>,
}

struct Row {
    cells: Vec<Cell>,
    in_head: bool,
}

fn render_table(table: &Selection<'_>, nested: bool, converter: &HtmlToMarkdown) -> Result<String> {
    let mut rows = Vec::new();
    let mut captions = Vec::new();
    let mut complex = nested;
    for child in table.children().iter() {
        if child.is("caption") {
            captions.push(converter.convert(&child.inner_html())?);
            continue;
        }
        let children = if child.is("tr") { child.clone() }
            else if child.is("thead,tbody,tfoot") { child.children().filter("tr") }
            else { continue; };
        for row in children.iter() {
            let mut cells = Vec::new();
            for cell in row.children().filter("th,td").iter() {
                let mut spans = Vec::new();
                for key in ["rowspan", "colspan"] {
                    if let Some(value) = cell.attr(key)
                        && value.trim().parse::<u64>().is_ok_and(|number| number != 1) {
                        spans.push(format!("{key}={}", value.trim()));
                    }
                }
                complex |= !spans.is_empty()
                    || cell.select("pre,ul,ol,dl,blockquote,urlmd-table").exists()
                    || cell.select("p").length() > 1;
                cells.push(Cell {
                    markdown: converter.convert(&cell.inner_html())?,
                    header: cell.is("th"),
                    row_header: cell.attr("scope").is_some_and(|scope| scope.eq_ignore_ascii_case("row") || scope.eq_ignore_ascii_case("rowgroup")),
                    spans,
                });
            }
            rows.push(Row { cells, in_head: child.is("thead") });
        }
    }
    complex |= rows.iter().filter(|row| row.in_head).count() > 1
        || rows.iter().skip(1).any(|row| row.in_head);
    let mut output = String::new();
    for caption in captions {
        if !caption.trim().is_empty() { output.push_str(caption.trim()); output.push_str("\n\n"); }
    }
    if complex {
        // GFM cannot express spans, multiple header rows or block content in a cell.
        // Keep DOM row/cell order with explicit span annotations instead of guessing a grid.
        for (r, row) in rows.iter().enumerate() {
            if row.cells.is_empty() { continue; }
            output.push_str(&format!("- **Row {}**\n", r + 1));
            for (c, cell) in row.cells.iter().enumerate() {
                let mut annotations = cell.spans.clone();
                if cell.header { annotations.insert(0, "header".into()); }
                let annotations = if annotations.is_empty() { String::new() }
                    else { format!(" ({})", annotations.join(", ")) };
                output.push_str(&format!("  - **Cell {}**{annotations}\n", c + 1));
                if !cell.markdown.trim().is_empty() {
                    output.push('\n');
                    for line in cell.markdown.trim().lines() {
                        output.push_str("    "); output.push_str(line); output.push('\n');
                    }
                }
                output.push('\n');
            }
        }
    } else {
        let columns = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
        if columns == 0 { return Ok(output); }
        let has_header = rows.first().is_some_and(|row| !row.cells.is_empty()
            && (row.in_head || row.cells.iter().all(|cell| cell.header && !cell.row_header)));
        let header = if has_header { rows.remove(0).cells } else { Vec::new() };
        append_row(&mut output, &header, columns);
        output.push('|');
        for _ in 0..columns { output.push_str(" --- |"); }
        output.push('\n');
        for row in rows { append_row(&mut output, &row.cells, columns); }
    }
    Ok(output)
}

fn append_row(output: &mut String, cells: &[Cell], columns: usize) {
    output.push('|');
    for c in 0..columns {
        let content = cells.get(c).map(|cell| cell.markdown.trim()).unwrap_or("");
        // Escaping pipes also works inside code spans; HTML entities do not decode there.
        let content = content.replace('\r', "").replace('\n', "<br>").replace('|', "\\|");
        output.push_str(" "); output.push_str(&content); output.push_str(" |");
    }
    output.push('\n');
}

/// Replace table subtrees bottom-up using an internal handler so Markdown emitted for
/// nested tables is not parsed as HTML or escaped a second time by the outer converter.
pub fn convert_fragment(fragment: &str) -> Result<String> {
    let rendered = Arc::new(Mutex::new(Vec::<String>::new()));
    let table_results = Arc::clone(&rendered);
    let converter = HtmlToMarkdown::builder()
        .options(Options { bullet_list_marker: BulletListMarker::Dash, ..Default::default() })
        .skip_tags(vec!["script", "style", "noscript", "template", "svg", "canvas"])
        .add_handler(vec!["urlmd-table"], move |_: &dyn htmd::element_handler::Handlers, element: htmd::Element| {
            let index = element.attrs.iter().find(|attr| attr.name.local.as_ref() == "data-index")?
                .value.parse::<usize>().ok()?;
            let content = table_results.lock().ok()?.get(index)?.clone();
            Some(format!("\n\n{content}\n\n").into())
        })
        .build();
    let doc = Document::from(fragment);
    // An input page cannot impersonate an internal replacement node.
    doc.select("urlmd-table").rename("div");
    let tables: Vec<_> = doc.select("table").iter().map(|table| {
        let nested = table.select("table").exists();
        (table, nested)
    }).collect();
    for (table, nested) in tables.into_iter().rev() {
        let markdown = render_table(&table, nested, &converter)?;
        let index = rendered.lock().unwrap().len();
        rendered.lock().unwrap().push(markdown);
        table.rename("urlmd-table");
        table.remove_all_attrs();
        table.set_attr("data-index", &index.to_string());
        table.set_html("");
    }
    Ok(converter.convert(&doc.select_single("body").inner_html())?)
}
