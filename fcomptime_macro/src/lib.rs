#![cfg_attr(
    feature = "nightly",
    feature(proc_macro_span, proc_macro_tracked_env, proc_macro_track_path)
)]

use proc_macro::TokenStream;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;


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

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static FILE_LOCK: Mutex<()> = Mutex::new(());

#[proc_macro_attribute]
pub fn info(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_str = item.to_string();

    let macro_start_line = proc_macro::Span::call_site().start().line();

    let _guard = FILE_LOCK.lock().unwrap();

    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let tree = parser.parse(&item_str, None).unwrap();
    let root_node = tree.root_node();

    let _ = std::fs::create_dir_all("./comptime");

    if !INITIALIZED.load(Ordering::SeqCst) {
        if let Ok(entries) = std::fs::read_dir("./comptime") {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_file() && entry.file_name().to_string_lossy().ends_with(".json") {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
        INITIALIZED.store(true, Ordering::SeqCst);
    }

    let func_query = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(function_item) @func",
    ).unwrap();
    let mut func_cursor = QueryCursor::new();
    let mut func_matches = func_cursor.matches(&func_query, root_node, item_str.as_bytes());

    let func_node = match func_matches.next().and_then(|m| m.captures.first()) {
        Some(capture) => capture.node,
        None => return item,
    };

    let mut func_name = String::new();
    if let Some(name_node) = func_node.child_by_field_name("name") {
        func_name = name_node.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
    }

    if func_name.is_empty() {
        return item;
    }

    let mut generics = Vec::new();
    let mut traits_list = Vec::new();

    if let Some(type_params) = func_node.child_by_field_name("type_parameters") {
        let mut tc = type_params.walk();
        for child in type_params.children(&mut tc) {
            if child.kind() == "type_parameter" || child.kind() == "constrained_type_parameter" {
                if let Some(id) = child.child_by_field_name("name").or_else(|| child.child(0)) {
                    let g_name = id.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
                    generics.push(g_name.clone());
                    
                    let mut child_c = child.walk();
                    for sub_child in child.children(&mut child_c) {
                        if sub_child.kind() == "type_bound" || sub_child.kind() == "trait_bounds" {
                            let bound_text = sub_child.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
                            traits_list.push(format!("{{\"generic\": \"{}\", \"bounds\": \"{}\"}}", g_name, bound_text.replace(':', "").trim()));
                        }
                    }
                }
            }
        }
    }

    let where_query = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(where_predicate left: (_) @left bounds: (_) @bounds)",
    ).unwrap();
    let mut where_cursor = QueryCursor::new();
    let mut where_matches = where_cursor.matches(&where_query, func_node, item_str.as_bytes());

    while let Some(wm) = where_matches.next() {
        let mut left_text = String::new();
        let mut bounds_text = String::new();
        for capture in wm.captures {
            let index = capture.index;
            let node = capture.node;
            if index == 0 {
                left_text = node.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
            } else if index == 1 {
                bounds_text = node.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
            }
        }
        if !left_text.is_empty() && !bounds_text.is_empty() {
            traits_list.push(format!("{{\"generic\": \"{}\", \"bounds\": \"{}\"}}", left_text, bounds_text));
        }
    }

    let mut parameters = Vec::new();
    if let Some(params) = func_node.child_by_field_name("parameters") {
        let mut tc = params.walk();
        for child in params.children(&mut tc) {
            if child.kind() == "parameter" {
                let p_name = child.child_by_field_name("pattern")
                    .map(|n| n.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string())
                    .unwrap_or_default();
                let p_type = child.child_by_field_name("type")
                    .map(|n| n.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string())
                    .unwrap_or_default();
                parameters.push((p_name, p_type));
            }
        }
    }

    let generics_json = generics.iter().map(|g| format!("\"{}\"", g)).collect::<Vec<_>>().join(", ");
    let where_json = traits_list.join(", ");
    let params_json = parameters.iter()
        .map(|(n, t)| format!("{{\"name\": \"{}\", \"type\": \"{}\"}}", n, t))
        .collect::<Vec<_>>()
        .join(", ");

    let target_path = format!("./comptime/{}.json", func_name);

    let mut existing_callers = String::new();
    if std::path::Path::new(&target_path).exists() {
        if let Ok(content) = std::fs::read_to_string(&target_path) {
            if let Some(start_idx) = content.find("\"callers\": [") {
                let part = &content[start_idx + 12..];
                if let Some(end_idx) = part.rfind(']') {
                    existing_callers = part[..end_idx].trim().to_string();
                }
            }
        }
    }

    let call_query = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(call_expression) @call",
    ).unwrap();
    let mut call_cursor = QueryCursor::new();
    let mut call_matches = call_cursor.matches(&call_query, root_node, item_str.as_bytes());

    let mut detected_callers = std::collections::HashMap::new();

    while let Some(m) = call_matches.next() {
        for capture in m.captures {
            let node = capture.node;
            let mut target_func = String::new();
            let mut generic_args = Vec::new();
            let mut val_exprs = Vec::new();

            let func_node = match node.child_by_field_name("function") {
                Some(f) => f,
                None => continue,
            };

            if func_node.kind() == "field_expression" {
                if let Some(method_node) = func_node.child_by_field_name("field") {
                    target_func = method_node.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
                }
                if let Some(receiver) = func_node.child_by_field_name("value") {
                    val_exprs.push(receiver.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string());
                }
            } else if func_node.kind() == "generic_function" {
                let base_func = func_node.child_by_field_name("function").or_else(|| func_node.child(0));
                if let Some(bf) = base_func {
                    if bf.kind() == "field_expression" {
                        if let Some(method_node) = bf.child_by_field_name("field") {
                            target_func = method_node.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
                        }
                        if let Some(receiver) = bf.child_by_field_name("value") {
                            val_exprs.push(receiver.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string());
                        }
                    } else {
                        target_func = bf.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
                    }
                }
                
                let mut nc = func_node.walk();
                if let Some(type_args_node) = func_node.children(&mut nc).find(|c| c.kind() == "type_arguments") {
                    let mut tc = type_args_node.walk();
                    for child in type_args_node.children(&mut tc) {
                        let kind = child.kind();
                        if kind != "<" && kind != ">" && kind != "," {
                            generic_args.push(child.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string());
                        }
                    }
                }
            } else {
                target_func = func_node.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string();
            }

            if let Some(args_node) = node.child_by_field_name("arguments") {
                let mut ac = args_node.walk();
                for child in args_node.children(&mut ac) {
                    let kind = child.kind();
                    if kind != "(" && kind != ")" && kind != "," {
                        val_exprs.push(child.utf8_text(item_str.as_bytes()).unwrap_or_default().trim().to_string());
                    }
                }
            }

            if target_func.is_empty() {
                continue;
            }

            let relative_line = node.start_position().row;
            let real_line = macro_start_line + relative_line;

            let gen_json = generic_args.iter().map(|g| format!("\"{}\"", g.replace('"', "\\\""))).collect::<Vec<_>>().join(", ");
            let val_json = val_exprs.iter().map(|v| format!("\"{}\"", v.replace('"', "\\\""))).collect::<Vec<_>>().join(", ");

            let caller_entry = format!(
                "{{\n      \"generics\": [{}],\n      \"values\": [{}],\n      \"line\": {}\n    }}",
                gen_json, val_json, real_line
            );

            detected_callers.entry(target_func).or_insert_with(Vec::new).push(caller_entry);
        }
    }

    for (t_func, callers) in detected_callers {
        let path = format!("./comptime/{}.json", t_func);
        let mut sub_existing = String::new();
        let mut sub_content = String::new();

        if std::path::Path::new(&path).exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                sub_content = content.clone();
                if let Some(start_idx) = content.find("\"callers\": [") {
                    let part = &content[start_idx + 12..];
                    if let Some(end_idx) = part.rfind(']') {
                        let trimmed = part[..end_idx].trim();
                        if !trimmed.is_empty() {
                            sub_existing.push_str(trimmed);
                            sub_existing.push_str(",\n    ");
                        }
                    }
                }
            }
        }

        sub_existing.push_str(&callers.join(",\n    "));

        let mut out_json = String::new();
        if !sub_content.is_empty() {
            if let Some(c_idx) = sub_content.find("\"callers\": [") {
                out_json.push_str(&sub_content[..c_idx + 12]);
                out_json.push_str("\n    ");
                out_json.push_str(&sub_existing);
                out_json.push_str("\n  ]\n}");
            }
        } else {
            out_json.push_str("{\n");
            out_json.push_str(&format!("  \"name\": \"{}\",\n", t_func));
            out_json.push_str("  \"line\": null,\n");
            out_json.push_str("  \"generics\": [],\n");
            out_json.push_str("  \"where\": [],\n");
            out_json.push_str("  \"parameters\": [],\n");
            out_json.push_str("  \"callers\": [\n    ");
            out_json.push_str(&sub_existing);
            out_json.push_str("\n  ]\n}");
        }

        let _ = std::fs::write(&path, out_json);
    }

    let mut final_content = String::new();
    let final_callers = existing_callers;

    if std::path::Path::new(&target_path).exists() {
        if let Ok(content) = std::fs::read_to_string(&target_path) {
            final_content = content;
        }
    }

    let mut out_json = String::new();
    if !final_content.is_empty() {
        if let Some(c_idx) = final_content.find("\"callers\": [") {
            out_json.push_str(&final_content[..c_idx]);
            out_json.push_str("\"line\": ");
            out_json.push_str(&macro_start_line.to_string());
            out_json.push_str(",\n  \"generics\": [");
            out_json.push_str(&generics_json);
            out_json.push_str("],\n  \"where\": [");
            out_json.push_str(&where_json);
            out_json.push_str("],\n  \"parameters\": [");
            out_json.push_str(&params_json);
            out_json.push_str("],\n  \"callers\": [\n    ");
            out_json.push_str(&final_callers);
            out_json.push_str("\n  ]\n}");
        }
    } else {
        out_json.push_str("{\n");
        out_json.push_str(&format!("  \"name\": \"{}\",\n", func_name));
        out_json.push_str(&format!("  \"line\": {},\n", macro_start_line));
        out_json.push_str(&format!("  \"generics\": [{}],\n", generics_json));
        out_json.push_str(&format!("  \"where\": [{}],\n", where_json));
        out_json.push_str(&format!("  \"parameters\": [{}],\n", params_json));
        out_json.push_str("  \"callers\": [\n    ");
        out_json.push_str(&final_callers);
        out_json.push_str("\n  ]\n}");
    }

    let _ = std::fs::write(&target_path, out_json);

    item
}