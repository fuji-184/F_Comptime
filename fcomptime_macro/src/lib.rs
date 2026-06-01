#![cfg_attr(
    feature = "nightly",
    feature(proc_macro_span, proc_macro_tracked_env, proc_macro_track_path)
)]

use proc_macro::TokenStream;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

fn resolve_target_file(span: proc_macro::Span, src_dir: &Path, start_line: usize) -> Option<PathBuf> {
    #[cfg(feature = "nightly")]
    {
        let path = PathBuf::from(span.source_file().path());
        if path.to_str().unwrap_or("").contains("_comptime.rs") {
            return None;
        }
        return Some(path);
    }

    #[cfg(not(feature = "nightly"))]
    {
        let _ = span;
        find_file_by_line(src_dir, start_line)
    }
}

fn register_file_dependency(path: &Path) {
    #[cfg(feature = "nightly")]
    {
        proc_macro::tracked_path::path(path.to_str().unwrap_or(""));
    }
    #[cfg(not(feature = "nightly"))]
    {
        let _ = path;
    }
}

fn get_env_var(name: &str) -> String {
    #[cfg(feature = "nightly")]
    {
        proc_macro::tracked_env::var(name).unwrap_or_default()
    }
    #[cfg(not(feature = "nightly"))]
    {
        std::env::var(name).unwrap_or_default()
    }
}

fn build_rerun_token(path: &Path, source: &str) -> TokenStream {
    #[cfg(feature = "nightly")]
    {
        let _ = (path, source);
        TokenStream::new()
    }
    #[cfg(not(feature = "nightly"))]
    {
        let mtime = fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            })
            .unwrap_or(0);

        let content_hash = source.bytes().fold(0u32, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(b as u32)
        });

        format!("const _: (u64, u32) = ({}, {});", mtime, content_hash)
            .parse()
            .unwrap_or_else(|_| TokenStream::new())
    }
}


#[proc_macro_attribute]
pub fn comptime(attr: TokenStream, item: TokenStream) -> TokenStream {
    let _ = attr;
    let span = proc_macro::Span::call_site();

    let manifest_dir = get_env_var("CARGO_MANIFEST_DIR");
    let src_dir = PathBuf::from(&manifest_dir).join("src");

    let item_str = item.to_string();

    let target_file = match resolve_target_file(span, &src_dir, span.start().line()) {
        Some(p) => p,
        None => return item,
    };

    register_file_dependency(&target_file);

    let source_code = fs::read_to_string(&target_file).unwrap_or_default();
    let rerun_token = build_rerun_token(&target_file, &source_code);

    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let file_tree = parser.parse(&source_code, None).unwrap();
    let root_node = file_tree.root_node();

    let item_tree = {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
        p.parse(&item_str, None).unwrap()
    };

    let base_line = span.start().line();

    let query_macro = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(macro_invocation macro: (identifier) @m_name (_) @m_body)",
    ).unwrap();

    let mut macro_cursor = QueryCursor::new();
    let mut macro_matches =
        macro_cursor.matches(&query_macro, item_tree.root_node(), item_str.as_bytes());

    let mut test_mods = String::new();

    while let Some(m) = macro_matches.next() {
        let mut macro_name = "";
        let mut body_node = None;
        let mut macro_node = None;

        for capture in m.captures {
            let cname = query_macro.capture_names()[capture.index as usize];
            if cname == "m_name" {
                macro_name = capture.node.utf8_text(item_str.as_bytes()).unwrap_or("");
                macro_node = Some(capture.node);
            } else if cname == "m_body" {
                body_node = Some(capture.node);
            }
        }

        if macro_name != "source" && macro_name != "async_source" {
            continue;
        }

        let body_node = match body_node {
            Some(b) => b,
            None => continue,
        };

        let m_node = macro_node.unwrap();
        
        let macro_relative_row = m_node.start_position().row;
        let mut call_line = base_line + macro_relative_row;

        if let Some(actual_line) = source_code.lines()
            .enumerate()
            .skip(base_line.saturating_sub(2)) 
            .take(macro_relative_row + 10)
            .find(|(_, line)| line.contains(&format!("{}!", macro_name)))
            .map(|(i, _)| i + 1) 
        {
            call_line = actual_line;
        }

        let body_text = body_node.utf8_text(item_str.as_bytes()).unwrap_or("").to_string();
        let mut body_lines = body_text.lines().collect::<Vec<&str>>();
        if body_lines.len() >= 2 {
            body_lines.remove(0);
            body_lines.pop();
        }

        let mut targets: Vec<String> = Vec::new();
        {
            let body_only = body_lines.join("\n");
            let mut p = Parser::new();
            p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
            let btree = p.parse(&body_only, None).unwrap();
            let q = Query::new(&tree_sitter_rust::LANGUAGE.into(), "(identifier) @name").unwrap();
            let mut qc = QueryCursor::new();
            let mut qm = qc.matches(&q, btree.root_node(), body_only.as_bytes());
            while let Some(mm) = qm.next() {
                for cap in mm.captures {
                    let name = cap.node.utf8_text(body_only.as_bytes()).unwrap_or("").to_string();
                    if name != "println" && !targets.contains(&name) {
                        targets.push(name);
                    }
                }
            }
        }

        let macro_call_byte = byte_offset_of_line(&source_code, call_line);

        let query_binding = Query::new(
            &tree_sitter_rust::LANGUAGE.into(),
            r#"[
                (let_declaration pattern: (_) @v_pattern) @v_stmt
                (const_item name: (identifier) @v_pattern) @v_stmt
                (static_item name: (identifier) @v_pattern) @v_stmt
            ]"#,
        ).unwrap();

        let mut all_entries: Vec<(usize, String)> = Vec::new();

        let mut bc = QueryCursor::new();
        let mut bm = bc.matches(&query_binding, root_node, source_code.as_bytes());
        while let Some(mm) = bm.next() {
            let mut stmt_node = None;
            let mut pattern_node = None;
            for cap in mm.captures {
                let cname = query_binding.capture_names()[cap.index as usize];
                if cname == "v_stmt" {
                    stmt_node = Some(cap.node);
                } else if cname == "v_pattern" {
                    pattern_node = Some(cap.node);
                }
            }
            let (stmt, pat) = match (stmt_node, pattern_node) {
                (Some(s), Some(p)) => (s, p),
                _ => continue,
            };
            let byte_start = stmt.start_byte();
            if byte_start >= macro_call_byte {
                continue;
            }
            let names = extract_names_from_pattern(pat, source_code.as_bytes());
            if names.iter().all(|n| !targets.contains(n)) {
                continue;
            }
            let stmt_text = stmt.utf8_text(source_code.as_bytes()).unwrap_or("").to_string();
            all_entries.push((byte_start, stmt_text));
        }

        let query_assign = Query::new(
            &tree_sitter_rust::LANGUAGE.into(),
            r#"[
                (assignment_expression left: (_) @a_left) @a_stmt
                (compound_assignment_expr left: (_) @a_left) @a_stmt
            ]"#,
        ).unwrap();

        let mut ac = QueryCursor::new();
        let mut am = ac.matches(&query_assign, root_node, source_code.as_bytes());
        while let Some(mm) = am.next() {
            let mut stmt_node = None;
            let mut left_node = None;
            for cap in mm.captures {
                let cname = query_assign.capture_names()[cap.index as usize];
                if cname == "a_stmt" {
                    stmt_node = Some(cap.node);
                } else if cname == "a_left" {
                    left_node = Some(cap.node);
                }
            }
            let (stmt, left) = match (stmt_node, left_node) {
                (Some(s), Some(l)) => (s, l),
                _ => continue,
            };
            let byte_start = stmt.start_byte();
            if byte_start >= macro_call_byte {
                continue;
            }
            let left_text = left.utf8_text(source_code.as_bytes()).unwrap_or("").to_string();
            let root_name = left_text.split('.').next().unwrap_or("").trim().to_string();
            if root_name.is_empty() || !targets.contains(&root_name) {
                continue;
            }
            if all_entries.iter().any(|(b, _)| *b == byte_start) {
                continue;
            }
            let parent = find_statement_parent(stmt);
            let stmt_text = parent.utf8_text(source_code.as_bytes()).unwrap_or("").to_string();
            all_entries.push((byte_start, stmt_text));
        }

        all_entries.sort_by_key(|(b, _)| *b);

        let mut found_definitions = String::new();
        for (_, text) in &all_entries {
            found_definitions.push_str("        ");
            found_definitions.push_str(text.trim());
            found_definitions.push('\n');
        }

        let mut extracted_body = String::new();
        for line in &body_lines {
            let inner = line.trim();
            if !inner.is_empty()
                && !inner.starts_with("//")
                && !inner.starts_with("/*")
            {
                extracted_body.push_str("        ");
                extracted_body.push_str(inner);
                extracted_body.push('\n');
            }
        }

        let test_fn_name = format!("comptime_line_{}", call_line);
        let test_mod = if macro_name == "async_source" {
            format!(
                "#[cfg(test)]\nmod {} {{\n    use super::*;\n    #[tokio::test]\n    async fn run() {{\n{}{}}}\n}}\n",
                test_fn_name,
                found_definitions,
                extracted_body,
            )
        } else {
            format!(
                "#[cfg(test)]\nmod {} {{\n    use super::*;\n    #[test]\n    fn run() {{\n{}{}}}\n}}\n",
                test_fn_name,
                found_definitions,
                extracted_body,
            )
        };
        test_mods.push_str(&test_mod);
    }

    let mut output = rerun_token;
    output.extend(item);
    let test_tokens: TokenStream = test_mods.parse().unwrap_or_else(|_| TokenStream::new());
    output.extend(test_tokens);
    output
}

fn extract_names_from_pattern(node: tree_sitter::Node, src: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    collect_identifiers(node, src, &mut names);
    names
}

fn collect_identifiers(node: tree_sitter::Node, src: &[u8], out: &mut Vec<String>) {
    if node.kind() == "identifier" {
        if let Ok(t) = node.utf8_text(src) {
            if t != "mut" {
                out.push(t.to_string());
            }
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifiers(child, src, out);
    }
}

fn find_statement_parent(node: tree_sitter::Node) -> tree_sitter::Node {
    let mut current = node;
    loop {
        if matches!(
            current.kind(),
            "expression_statement" | "let_declaration" | "const_item" | "static_item"
        ) {
            return current;
        }
        match current.parent() {
            Some(p) => current = p,
            None => return node,
        }
    }
}

fn byte_offset_of_line(source: &str, target_line: usize) -> usize {
    let mut offset = 0;
    for (i, line) in source.lines().enumerate() {
        if i + 1 == target_line {
            return offset;
        }
        offset += line.len() + 1;
    }
    offset
}

#[cfg(not(feature = "nightly"))]
fn find_file_by_line(base_dir: &Path, target_line: usize) -> Option<PathBuf> {
    let mut dirs = vec![base_dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.is_file()
                    && path.extension().map_or(false, |e| e == "rs")
                    && !path.to_str().unwrap_or("").contains("_comptime.rs")
                {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if content.contains("source!")
                            && content.lines().count() >= target_line
                        {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

#[proc_macro]
pub fn comptime_token(input: TokenStream) -> TokenStream {
    let name = input.to_string();
    let name = name.trim().trim_matches('"');
    let path = format!("./comptime/{}", name);

    #[cfg(feature = "nightly")]
    proc_macro::tracked_path::path(&path);

    let content = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "comptime file '{}' not found, run cargo test --features=comptime first",
            name
        )
    });

    content
        .parse()
        .unwrap_or_else(|_| panic!("failed to parse comptime file '{}'", name))
}

fn replace_placeholder(source: &str, placeholder: &str, value: &str) -> String {
    let mut result = String::new();
    let mut idx = 0;
    let ph_len = placeholder.len();

    while idx < source.len() {
        if source[idx..].starts_with(placeholder) {
            let after = source[idx + ph_len..].chars().next();
            let after_ok = match after {
                None => true,
                Some(c) => !c.is_ascii_digit(),
            };

            if after_ok {
                result.push_str(value);
                idx += ph_len;
            } else {
                result.push(source[idx..].chars().next().unwrap());
                idx += source[idx..].chars().next().unwrap().len_utf8();
            }
        } else {
            result.push(source[idx..].chars().next().unwrap());
            idx += source[idx..].chars().next().unwrap().len_utf8();
        }
    }
    result
}

#[proc_macro]
pub fn comptime_type(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();
    
    let input_str = input_str.trim();
    let comma_idx = match input_str.find(',') {
        Some(i) => i,
        None => panic!("comptime_type!: expected format: \"name\", <item>"),
    };
    
    let name_part = input_str[..comma_idx].trim().trim_matches('"');
    let item_part = input_str[comma_idx + 1..].trim();
    
    let path = format!("./comptime/{}", name_part);

    #[cfg(feature = "nightly")]
    proc_macro::tracked_path::path(&path);

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            let wrapped = format!("#[cfg(not(test))] {}", item_part);
            return wrapped.parse().unwrap_or_else(|_| TokenStream::new());
        }
    };

    let parts: Vec<&str> = content.trim().split(',').collect();

    let mut result = item_part.to_string();
    for (i, part) in parts.iter().enumerate().rev() {
        let placeholder = format!("#{}", i + 1);
        result = replace_placeholder(&result, &placeholder, part.trim());
    }

    result.parse().unwrap_or_else(|_| TokenStream::new())
}
