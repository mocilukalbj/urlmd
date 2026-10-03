//! Run scripts/check_sites.py first, then opt in with URLMD_SITE_SNAPSHOTS and --ignored.
use std::{collections::HashMap, fs, path::PathBuf};
use dom_query::Document;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};
use url::Url;
use urlmd::html_to_markdown;

fn code_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut code = None;
    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code = Some(String::new()),
            Event::Text(text) => if let Some(code) = &mut code { code.push_str(&text); },
            Event::End(TagEnd::CodeBlock) => blocks.push(code.take().unwrap()),
            _ => {},
        }
    }
    blocks
}

fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace() && *c != '\u{200b}').collect()
}

fn visible(markdown: &str) -> String {
    let mut text = String::new();
    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            _ => {},
        }
    }
    compact(&text)
}

#[test]
#[ignore = "requires public-site snapshots; see scripts/check_sites.py"]
fn saved_sites_preserve_code_and_table_cells() {
    let root = PathBuf::from(std::env::var_os("URLMD_SITE_SNAPSHOTS").expect("set URLMD_SITE_SNAPSHOTS"));
    let report: Value = serde_json::from_slice(&fs::read(root.join("report.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    for case in report["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        assert_eq!(case["status"], "passed", "{name}: smoke test did not pass");
        let folder = root.join(name);
        let keep = fs::read_to_string(folder.join("keep.md")).unwrap();
        let codes = code_blocks(&keep);
        for mode in ["relative", "text"] {
            let md = fs::read_to_string(folder.join(format!("{mode}.md"))).unwrap();
            assert_eq!(codes, code_blocks(&md), "{name}: {mode} changed code blocks");
        }
        let mut baseline_checked = false;
        let mut newline_fixes = 0;
        // TypeScript's old blocks concatenated line DIVs and UI; they intentionally change.
        if name != "typescript" && folder.join("baseline.md").exists() {
            let old = code_blocks(&fs::read_to_string(folder.join("baseline.md")).unwrap());
            let mut remaining = HashMap::new();
            for code in &codes { *remaining.entry(code).or_insert(0usize) += 1; }
            for code in &old {
                let matched = remaining.iter().find(|(candidate, count)| **count > 0 && ***candidate == *code)
                    .map(|(candidate, _)| *candidate)
                    .or_else(|| remaining.iter().find(|(candidate, count)| **count > 0
                        && candidate.replace('\n', "") == code.replace('\n', ""))
                        .map(|(candidate, _)| { newline_fixes += 1; *candidate }));
                let matched = matched.unwrap_or_else(|| panic!("{name}: lost or changed old code block: {code:.150}"));
                *remaining.get_mut(matched).unwrap() -= 1;
            }
            baseline_checked = true;
        }
        let mut cells_checked = 0;
        if case["content_type"].as_str().unwrap().contains("html") {
            let html = fs::read_to_string(folder.join("source.html")).unwrap();
            let base = Url::parse(case["final_url"].as_str().unwrap()).unwrap();
            let extraction = html_to_markdown(&html, Some(&base), None, false).unwrap();
            assert_eq!(keep, extraction.markdown, "{name}: snapshots must match current converter");
            let doc = Document::from(html.as_str());
            doc.select("script,style,svg,[hidden],[aria-hidden='true'],nav,footer,[role='navigation'],#docContent > .navheader,#docContent > .navfooter").remove();
            for element in doc.select("[style]").iter() {
                let style = compact(&element.attr("style").unwrap()).to_ascii_lowercase();
                if style.contains("display:none") || style.contains("visibility:hidden") {
                    element.remove();
                }
            }
            let selected = doc.select(&extraction.selection);
            let text = visible(&keep);
            for cell in selected.select("th,td").iter() {
                if cell.select("table").exists() { continue; }
                let value = compact(&cell.text());
                if value.is_empty() { continue; }
                assert!(text.contains(&value), "{name}: table cell missing: {value:.150}");
                cells_checked += 1;
            }
        }
        if name.starts_with("mdn-") {
            assert!(!keep.contains("This feature is well established"), "{name}: Baseline UI leaked");
        }
        if name == "rust-vec" {
            let text = fs::read_to_string(folder.join("text.md")).unwrap();
            assert!(!text.lines().take(12).any(|line| line.trim() == "Write"));
            assert!(text.contains("Write for Vec"));
        }
        if name == "typescript" {
            assert!(keep.starts_with("# Everyday Types"));
            assert!(codes.iter().any(|code| code.starts_with("let obj: any = { x: 0 };\n//")));
            assert!(codes.iter().all(|code| !code.ends_with("Try\n")));
            let html = fs::read_to_string(folder.join("source.html")).unwrap();
            let doc = Document::from(html.as_str());
            let examples = doc.select("#handbook-content pre");
            assert_eq!(examples.length(), codes.len(), "TypeScript code block count");
            for pre in examples.iter() {
                let lines = pre.select_single("code").children().filter("div.line");
                if lines.exists() {
                    let expected = lines.iter().map(|line| line.text().to_string()).collect::<Vec<_>>().join("\n");
                    assert!(codes.iter().any(|code| code.trim_end() == expected.trim_end()), "TypeScript code changed: {expected:.150}");
                }
            }
        }
        results.push(json!({"name": name, "code_blocks": codes.len(), "table_cells_checked": cells_checked,
                            "baseline_code_preserved": baseline_checked, "baseline_blocks_with_restored_newlines": newline_fixes}));
    }
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
    fs::write(root.join("structural-checks.json"), serde_json::to_vec_pretty(&results).unwrap()).unwrap();
}
