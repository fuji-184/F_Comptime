#![cfg_attr(
    feature = "nightly",
    feature(proc_macro_span, proc_macro_tracked_env, proc_macro_track_path)
)]

use proc_macro::TokenStream;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};
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
        use std::hash::{Hash, Hasher};

        let mtime_nanos = fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64
            })
            .unwrap_or(0);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        let content_hash = hasher.finish();

        format!("const _: (u64, u64) = ({}, {});", mtime_nanos, content_hash)
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

        let expected_line = base_line + 1 + macro_relative_row;
        let mut call_line = expected_line;
        let mut best_dist = usize::MAX;

        for (i, line) in source_code
            .lines()
            .enumerate()
            .skip(base_line.saturating_sub(1))
            .take(macro_relative_row + 8)
        {
            if line.contains(&format!("{}!", macro_name)) {
                let l = i + 1;
                let d = l.abs_diff(expected_line);
                if d < best_dist {
                    best_dist = d;
                    call_line = l;
                }
            }
        }

        let body_text = body_node.utf8_text(item_str.as_bytes()).unwrap_or("").to_string();
        let inner = match (body_text.find('{'), body_text.rfind('}')) {
            (Some(s), Some(e)) if e > s => &body_text[s + 1..e],
            _ => body_text.as_str(),
        };
        let body_lines = inner.lines().collect::<Vec<&str>>();

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
                    let node = cap.node;
                    let mut skip = false;
                    if let Some(par) = node.parent() {
                        match par.kind() {
                            "call_expression" | "generic_function" => {
                                if let Some(f) = par.child_by_field_name("function") {
                                    if f == node {
                                        skip = true;
                                    }
                                }
                            }
                            "field_expression" => {
                                if let Some(f) = par.child_by_field_name("field") {
                                    if f == node {
                                        skip = true;
                                    }
                                }
                            }
                            "macro_invocation" => {
                                skip = true;
                            }
                            _ => {}
                        }
                    }
                    if skip {
                        continue;
                    }
                    let name = node.utf8_text(body_only.as_bytes()).unwrap_or("").to_string();
                    if !is_framework_name(&name) && !targets.contains(&name) {
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
            let left_text = root_identifier(left, source_code.as_bytes());
            if left_text.is_empty() || left_text == "self" || !targets.contains(&left_text) {
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

        let test_fn_name = format!("comptime_line_{}_{}", call_line, macro_relative_row);
        let test_mod = if macro_name == "async_source" {
            format!(
                "#[cfg(test)]\nmod {} {{\n    use super::*;\n    use fcomptime::tokio as tokio;\n    #[tokio::test]\n    async fn run() {{\n{}{}}}\n}}\n",
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

fn is_framework_name(name: &str) -> bool {
    matches!(
        name,
        "source"
            | "async_source"
            | "output"
            | "call"
            | "call_scope"
            | "println"
            | "print"
            | "eprintln"
            | "eprint"
            | "panic"
            | "dbg"
            | "assert"
            | "assert_eq"
            | "assert_ne"
            | "assert_neq"
    )
}

fn root_identifier(node: tree_sitter::Node, src: &[u8]) -> String {
    if node.kind() == "identifier" {
        return node.utf8_text(src).unwrap_or("").trim().to_string();
    }
    if let Some(child) = node.child(0) {
        return root_identifier(child, src);
    }
    node.utf8_text(src).unwrap_or("").trim().to_string()
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
    for (i, line) in source.split('\n').enumerate() {
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
    for extra in ["tests", "examples", "benches"] {
        let p = base_dir.join(extra);
        if p.is_dir() {
            dirs.push(p);
        }
    }

    let mut candidates: Vec<(PathBuf, usize)> = Vec::new();

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
                        let has_macro =
                            content.contains("source!") || content.contains("async_source!");
                        if has_macro && content.lines().count() >= target_line {
                            let dist = content
                                .lines()
                                .enumerate()
                                .filter(|(_, l)| {
                                    l.trim_start().starts_with("#[") && l.contains("comptime")
                                })
                                .map(|(i, _)| target_line.saturating_sub(i + 1))
                                .min()
                                .unwrap_or(usize::MAX);
                            candidates.push((path, dist));
                        }
                    }
                }
            }
        }
    }

    candidates.sort_by_key(|(_, d)| *d);
    candidates.first().map(|(p, _)| p.clone())
}

#[proc_macro]
pub fn comptime_token(input: TokenStream) -> TokenStream {
    let name = input.to_string();
    let name = name.trim().trim_matches('"');
    let path = PathBuf::from(get_env_var("CARGO_MANIFEST_DIR"))
        .join("comptime")
        .join(name);

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

fn replace_placeholders(source: &str, parts: &[&str]) -> String {
    let mut result = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '#' {
            let mut done = false;
            for k in (1..=parts.len()).rev() {
                let ph: Vec<char> = format!("#{}", k).chars().collect();
                if chars.len() - i >= ph.len() && &chars[i..i + ph.len()] == &ph[..] {
                    let after_ok = chars
                        .get(i + ph.len())
                        .map_or(true, |c| !c.is_ascii_digit());
                    if after_ok {
                        result.push_str(parts[k - 1]);
                        i += ph.len();
                        done = true;
                        break;
                    }
                }
            }
            if !done {
                result.push('#');
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
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
    
    let path = PathBuf::from(get_env_var("CARGO_MANIFEST_DIR"))
        .join("comptime")
        .join(name_part);

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

    let result = replace_placeholders(item_part, &parts);

    result.parse().unwrap_or_else(|_| TokenStream::new())
}

static FILE_LOCK: Mutex<()> = Mutex::new(());

#[proc_macro_attribute]
pub fn info(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_str = item.to_string();
    let macro_start_line = proc_macro::Span::call_site().start().line();
    let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let comptime_dir = PathBuf::from(get_env_var("CARGO_MANIFEST_DIR")).join("comptime");
    let _ = std::fs::create_dir_all(&comptime_dir);

    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let tree = parser.parse(&item_str, None).unwrap();
    let root_node = tree.root_node();

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
    
    let mut return_type = String::from("()");
    if let Some(type_node) = func_node.child_by_field_name("return_type") {
        return_type = type_node.utf8_text(item_str.as_bytes()).unwrap_or("()").trim().to_string();
        return_type = return_type.trim_start_matches("->").trim().to_string();
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
                            traits_list.push(serde_json::json!({
                                "generic": g_name,
                                "bounds": bound_text.replace(':', "").trim().to_string()
                            }));
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
            let result = if let Some(stripped) = bounds_text.strip_prefix(':') {
                stripped.trim_start()
            } else {
                &bounds_text
            };
            traits_list.push(serde_json::json!({
                "generic": left_text,
                "bounds": result.to_string()
            }));
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
                parameters.push(serde_json::json!({
                    "name": p_name,
                    "type": p_type
                }));
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

            let caller_entry = serde_json::json!({
                "generics": generic_args,
                "values": val_exprs,
                "line": real_line
            });

            detected_callers.entry(target_func).or_insert_with(Vec::new).push(caller_entry);
        }
    }

    for (t_func, mut callers) in detected_callers {
        let path = comptime_dir.join(format!("{}.json", t_func));
        let mut doc: serde_json::Value = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        if doc.get("name").is_none() {
            doc["name"] = serde_json::Value::String(t_func.clone());
            doc["line"] = serde_json::Value::Null;
            doc["generics"] = serde_json::Value::Array(Vec::new());
            doc["where"] = serde_json::Value::Array(Vec::new());
            doc["parameters"] = serde_json::Value::Array(Vec::new());
            doc["return_type"] = serde_json::Value::Null;
            doc["callers"] = serde_json::Value::Array(Vec::new());
        }

        if let Some(existing_callers) = doc["callers"].as_array_mut() {
            for caller in &mut callers {
                if !existing_callers.contains(caller) {
                    existing_callers.push(caller.take());
                }
            }
        }

        if let Ok(out_json) = serde_json::to_string_pretty(&doc) {
            let _ = std::fs::write(&path, out_json);
        }
    }

    let target_path = comptime_dir.join(format!("{}.json", func_name));
    let mut target_doc: serde_json::Value = if std::path::Path::new(&target_path).exists() {
        std::fs::read_to_string(&target_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    target_doc["name"] = serde_json::Value::String(func_name);
    target_doc["line"] = serde_json::json!(macro_start_line);
    target_doc["generics"] = serde_json::json!(generics);
    target_doc["where"] = serde_json::json!(traits_list);
    target_doc["parameters"] = serde_json::json!(parameters);
    target_doc["return_type"] = serde_json::Value::String(return_type);
    if target_doc.get("callers").is_none() {
        target_doc["callers"] = serde_json::Value::Array(Vec::new());
    }

    if let Ok(out_json) = serde_json::to_string_pretty(&target_doc) {
        let _ = std::fs::write(&target_path, out_json);
    }

    item
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_basic_and_double_digit() {
        let parts = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
        assert_eq!(replace_placeholders("#1 + #2", &parts), "a + b");
        assert_eq!(replace_placeholders("#10 and #2", &parts), "j and b");
        assert_eq!(replace_placeholders("#12", &parts), "l");
        assert_eq!(replace_placeholders("(#1, #10, #11)", &parts), "(a, j, k)");
    }

    #[test]
    fn placeholder_respects_digit_boundary() {
        let parts = ["a", "b"];
        assert_eq!(replace_placeholders("#12", &parts), "#12");
        assert_eq!(replace_placeholders("x#1y", &parts), "xay");
    }

    #[test]
    fn byte_offset_crlf() {
        let src = "a\r\nbb\r\nccc";
        assert_eq!(byte_offset_of_line(src, 1), 0);
        assert_eq!(byte_offset_of_line(src, 2), 3);
        assert_eq!(byte_offset_of_line(src, 3), 7);
    }

    #[test]
    fn byte_offset_lf() {
        let src = "a\nbb\nccc";
        assert_eq!(byte_offset_of_line(src, 1), 0);
        assert_eq!(byte_offset_of_line(src, 2), 2);
        assert_eq!(byte_offset_of_line(src, 3), 5);
    }
}
