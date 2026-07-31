#![cfg_attr(
    feature = "nightly",
    feature(proc_macro_span, proc_macro_tracked_env, proc_macro_track_path)
)]

use proc_macro::TokenStream;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};
use std::sync::Mutex;

const FRAMEWORK_MACROS: [&str; 16] = [
    "source",
    "async_source",
    "output",
    "call",
    "call_scope",
    "func",
    "comptime_source",
    "parse",
    "assign",
    "comptime_token",
    "comptime_type",
    "comptime_include",
    "comptime_include_expr",
    "handle_default",
    "init_comptime",
    "get",
];


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

    let item_fn_name = {
        let q = Query::new(
            &tree_sitter_rust::LANGUAGE.into(),
            "(function_item name: (identifier) @n)",
        );
        match q {
            Ok(q) => {
                let mut c = QueryCursor::new();
                let mut m = c.matches(&q, item_tree.root_node(), item_str.as_bytes());
                m.next().and_then(|mm| mm.captures.first())
                    .map(|cap| cap.node.utf8_text(item_str.as_bytes()).unwrap_or("").to_string())
                    .unwrap_or_default()
            }
            Err(_) => String::new(),
        }
    };

    let is_method = {
        let mut found = false;
        if let Ok(q) = Query::new(&tree_sitter_rust::LANGUAGE.into(), "(function_item) @f") {
            let mut c = QueryCursor::new();
            let mut m = c.matches(&q, root_node, source_code.as_bytes());
            while let Some(mm) = m.next() {
                for cap in mm.captures {
                    let node = cap.node;
                    let row = node.start_position().row + 1;
                    if row.abs_diff(base_line) > 2 {
                        continue;
                    }
                    let name = node
                        .child_by_field_name("name")
                        .and_then(|n| n.utf8_text(source_code.as_bytes()).ok())
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() && name != item_fn_name {
                        continue;
                    }
                    let mut cur = node.parent();
                    while let Some(p) = cur {
                        if p.kind() == "impl_item" {
                            found = true;
                            break;
                        }
                        cur = p.parent();
                    }
                }
            }
        }
        found
    };

    let (fn_params, _fn_bindable) = fn_params_and_bindable(&item_str, item_tree.root_node());

    let body_block = {
        let q = Query::new(
            &tree_sitter_rust::LANGUAGE.into(),
            "(function_item body: (block) @b)",
        );
        match q {
            Ok(q) => {
                let mut c = QueryCursor::new();
                let mut m = c.matches(&q, item_tree.root_node(), item_str.as_bytes());
                m.next().and_then(|mm| mm.captures.first()).map(|cap| cap.node)
            }
            Err(_) => None,
        }
    };

    let query_macro = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(macro_invocation macro: (identifier) @m_name (_) @m_body)",
    ).unwrap();

    let mut macro_cursor = QueryCursor::new();
    let mut macro_matches =
        macro_cursor.matches(&query_macro, item_tree.root_node(), item_str.as_bytes());

    let mut test_mods = String::new();
    let mut errors: Vec<String> = Vec::new();

    if is_method {
        errors.push(
            "comptime: #[comptime] on impl methods is not supported (the generated test module \
             cannot be placed inside an impl block); extract the computation into a free fn and \
             call it from the method"
                .to_string(),
        );
    }

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

        if is_method {
            continue;
        }

        if !fn_params.is_empty() {
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
        let body_only = body_lines.join("\n");

        let mut targets: Vec<String> = Vec::new();
        {
            let body_only = body_only.clone();
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

        let item_call_byte = match body_block {
            Some(bb) => {
                let mut cur = m_node;
                loop {
                    match cur.parent() {
                        Some(p) if p.id() == bb.id() => break Some((cur.start_byte(), cur.end_byte())),
                        Some(p) => cur = p,
                        None => break None,
                    }
                }
            }
            None => None,
        };

        let mut call_errors: Vec<String> = Vec::new();

        if let Some(bb) = body_block {
            if let Err(e) = check_source_at_fn_top(m_node, bb) {
                call_errors.push(e);
            }
        }

        let prefix_text = fn_prefix_text(&item_str, body_block, item_call_byte.map(|(s, _)| s));
        let prefix_bound = bound_names_in_str(&prefix_text);

        if has_self_in_str(&body_only) || has_self_in_str(&prefix_text) {
            call_errors.push(format!(
                "comptime error (line {}): source! body or captured prefix references `self`; \
                 `self` cannot be captured at compile time — extract the needed fields into locals \
                 before source!, e.g. `let x = self.x;`",
                call_line
            ));
        }

        let after_names = names_bound_after(&item_str, body_block, item_call_byte.map(|(_, e)| e));
        for t in &targets {
            if after_names.contains(t) && !prefix_bound.contains(t) {
                call_errors.push(format!(
                    "comptime error (line {}): source! body references `{}` which is defined after \
                     the source! call; move its definition before source! so it can be captured",
                    call_line, t
                ));
            }
        }

        if !call_errors.is_empty() {
            errors.extend(call_errors);
            continue;
        }

        let module_consts = module_consts_for(
            root_node,
            source_code.as_bytes(),
            &targets.iter().cloned().collect(),
        );

        let mut setup = String::new();
        if !module_consts.is_empty() {
            for c in &module_consts {
                setup.push_str("        ");
                setup.push_str(c.trim());
                setup.push('\n');
            }
        }
        setup.push_str(&prefix_text);

        let test_fn_name = format!("comptime_line_{}_{}", call_line, macro_relative_row);
        let test_mod = if macro_name == "async_source" {
            format!(
                "#[cfg(test)]\nmod {} {{\n    use super::*;\n    use fcomptime::tokio as tokio;\n    #[tokio::test]\n    async fn run() {{\n{}{}}}\n}}\n",
                test_fn_name,
                setup,
                extracted_body,
            )
        } else {
            format!(
                "#[cfg(test)]\nmod {} {{\n    use super::*;\n    #[allow(unused_variables)]\n    #[test]\n    fn run() {{\n{}{}}}\n}}\n",
                test_fn_name,
                setup,
                extracted_body,
            )
        };
        test_mods.push_str(&test_mod);
    }

    let mut output = if is_method {
        TokenStream::new()
    } else {
        rerun_token
    };
    output.extend(item);
    let test_tokens: TokenStream = test_mods.parse().unwrap_or_else(|_| TokenStream::new());
    output.extend(test_tokens);
    for e in &errors {
        let msg = format!("compile_error!({:?});", e);
        if let Ok(ts) = msg.parse::<TokenStream>() {
            output.extend(ts);
        }
    }
    output
}

fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            cur.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            c if c == sep && depth == 0 => {
                out.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn fn_params_and_bindable(item_str: &str, root: Node) -> (Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let mut bindable = Vec::new();
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(function_item parameters: (parameters) @p)",
    ) else {
        return (names, bindable);
    };
    let mut c = QueryCursor::new();
    let mut m = c.matches(&q, root, item_str.as_bytes());
    while let Some(mm) = m.next() {
        for cap in mm.captures {
            let params_node = cap.node;
            let mut cc = params_node.walk();
            for child in params_node.children(&mut cc) {
                match child.kind() {
                    "parameter" => {
                        if let Some(pat) = child.child_by_field_name("pattern") {
                            if pat.kind() == "identifier" {
                                let n = pat
                                    .utf8_text(item_str.as_bytes())
                                    .unwrap_or("")
                                    .trim()
                                    .to_string();
                                if !n.is_empty() {
                                    names.push(n.clone());
                                    bindable.push(n);
                                }
                            } else {
                                let mut ids = Vec::new();
                                collect_identifiers(pat, item_str.as_bytes(), &mut ids);
                                names.extend(ids);
                            }
                        }
                    }
                    "self_parameter" => names.push("self".to_string()),
                    _ => {}
                }
            }
        }
    }
    (names, bindable)
}

fn check_source_at_fn_top(m_node: Node, body_block: Node) -> Result<(), String> {
    let mut cur = m_node.parent();
    while let Some(p) = cur {
        if p.kind() == "block" {
            if p.id() != body_block.id() {
                return Err(
                    "comptime error: source! must be called at the top level of the #[comptime] \
                     fn body (not inside if/loop/nested block), because values there may depend \
                     on runtime control flow"
                        .to_string(),
                );
            }
            return Ok(());
        }
        cur = p.parent();
    }
    Err("comptime error: cannot locate source! within the #[comptime] fn body".to_string())
}

fn fn_prefix_text(item_str: &str, body_block: Option<Node>, call_stmt_start: Option<usize>) -> String {
    let Some(bb) = body_block else {
        return String::new();
    };
    let Some(bound) = call_stmt_start else {
        return String::new();
    };
    let mut out = String::new();
    let mut c = bb.walk();
    for stmt in bb.children(&mut c) {
        let kind = stmt.kind();
        if kind == "{" || kind == "}" {
            continue;
        }
        if stmt.start_byte() >= bound {
            continue;
        }
        if is_framework_statement(stmt, item_str.as_bytes()) {
            continue;
        }
        if let Ok(text) = stmt.utf8_text(item_str.as_bytes()) {
            for line in text.lines() {
                out.push_str("        ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

fn is_framework_statement(stmt: Node, src: &[u8]) -> bool {
    let mac = if stmt.kind() == "macro_invocation" {
        Some(stmt)
    } else if stmt.kind() == "expression_statement" {
        stmt.child(0).filter(|c| c.kind() == "macro_invocation")
    } else {
        None
    };
    if let Some(m) = mac {
        if let Some(name_node) = m.child_by_field_name("macro") {
            if let Ok(name) = name_node.utf8_text(src) {
                return FRAMEWORK_MACROS.contains(&name);
            }
        }
    }
    false
}

fn identifiers_in_str(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(text, None) else {
        return out;
    };
    let Ok(q) = Query::new(&tree_sitter_rust::LANGUAGE.into(), "(identifier) @name") else {
        return out;
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, tree.root_node(), text.as_bytes());
    while let Some(mm) = qm.next() {
        for cap in mm.captures {
            if let Ok(n) = cap.node.utf8_text(text.as_bytes()) {
                if !out.iter().any(|x| x == n) {
                    out.push(n.to_string());
                }
            }
        }
    }
    out
}

fn bound_names_in_str(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(text, None) else {
        return out;
    };
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "[(let_declaration pattern: (_) @p) (const_item name: (identifier) @c) (static_item name: (identifier) @s)]",
    ) else {
        return out;
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, tree.root_node(), text.as_bytes());
    while let Some(mm) = qm.next() {
        for cap in mm.captures {
            let names = match cap.node.kind() {
                "identifier" => vec![cap.node.utf8_text(text.as_bytes()).unwrap_or("").to_string()],
                _ => extract_names_from_pattern(cap.node, text.as_bytes()),
            };
            out.extend(names);
        }
    }
    out
}

fn has_self_in_str(text: &str) -> bool {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(text, None) else {
        return false;
    };
    if let Ok(q) = Query::new(&tree_sitter_rust::LANGUAGE.into(), "(self) @s") {
        let mut qc = QueryCursor::new();
        let mut qm = qc.matches(&q, tree.root_node(), text.as_bytes());
        if qm.next().is_some() {
            return true;
        }
    }
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|t| t == "self")
}

fn names_bound_after(
    item_str: &str,
    body_block: Option<Node>,
    call_stmt_end: Option<usize>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(bb) = body_block else {
        return out;
    };
    let Some(bound) = call_stmt_end else {
        return out;
    };
    let Ok(q_let) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "[(let_declaration pattern: (_) @p) (const_item name: (identifier) @c) (static_item name: (identifier) @s)]",
    ) else {
        return out;
    };
    let Ok(q_assign) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "[(assignment_expression left: (_) @l) (compound_assignment_expr left: (_) @l)]",
    ) else {
        return out;
    };
    let mut c = bb.walk();
    for stmt in bb.children(&mut c) {
        if stmt.start_byte() <= bound {
            continue;
        }
        let mut qc = QueryCursor::new();
        let mut qm = qc.matches(&q_let, stmt, item_str.as_bytes());
        while let Some(mm) = qm.next() {
            for cap in mm.captures {
                let names = match cap.node.kind() {
                    "identifier" => vec![cap.node.utf8_text(item_str.as_bytes()).unwrap_or("").to_string()],
                    _ => extract_names_from_pattern(cap.node, item_str.as_bytes()),
                };
                out.extend(names);
            }
        }
        let mut qc2 = QueryCursor::new();
        let mut qm2 = qc2.matches(&q_assign, stmt, item_str.as_bytes());
        while let Some(mm) = qm2.next() {
            for cap in mm.captures {
                let name = root_identifier(cap.node, item_str.as_bytes());
                if !name.is_empty() && name != "self" {
                    out.insert(name);
                }
            }
        }
    }
    out
}

fn module_level_consts(root: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "[(const_item) (static_item)] @item",
    ) else {
        return out;
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, root, src);
    while let Some(mm) = qm.next() {
        for cap in mm.captures {
            let node = cap.node;
            if let Some(parent) = node.parent() {
                if parent.kind() != "source_file" {
                    continue;
                }
            }
            let name = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(src).ok())
                .unwrap_or("")
                .to_string();
            let text = node.utf8_text(src).unwrap_or("").to_string();
            if !name.is_empty() {
                out.push((name, text));
            }
        }
    }
    out
}

fn module_consts_for(root: Node, src: &[u8], seed: &HashSet<String>) -> Vec<String> {
    let all = module_level_consts(root, src);
    let mut seed = seed.clone();
    let mut chosen: HashSet<String> = HashSet::new();
    loop {
        let mut added = false;
        for (name, text) in &all {
            if chosen.contains(name) {
                continue;
            }
            let idents = identifiers_in_str(text);
            if seed.contains(name) || idents.iter().any(|i| seed.contains(i)) {
                chosen.insert(name.clone());
                seed.insert(name.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    all.iter()
        .filter(|(n, _)| chosen.contains(n))
        .map(|(_, t)| t.clone())
        .collect()
}

fn compile_error_ts(msg: &str) -> TokenStream {
    compile_error_string(msg).parse().unwrap_or_default()
}

fn compile_error_string(msg: &str) -> String {
    format!("compile_error!({:?});", msg)
}

fn is_framework_name(name: &str) -> bool {
    matches!(
        name,
        "source"
            | "async_source"
            | "output"
            | "call"
            | "call_scope"
            | "func"
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

#[allow(dead_code)]
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
pub fn comptime_include(input: TokenStream) -> TokenStream {
    comptime_include_inner(input, true)
}

#[proc_macro]
pub fn comptime_include_expr(input: TokenStream) -> TokenStream {
    comptime_include_inner(input, false)
}

fn comptime_include_inner(input: TokenStream, item_ctx: bool) -> TokenStream {
    let input_str = input.to_string();
    let (name, default) = match input_str.find(',') {
        Some(i) => (input_str[..i].trim(), input_str[i + 1..].trim()),
        None => (input_str.trim(), ""),
    };
    let name = name.trim_matches('"');
    if name.is_empty() {
        return compile_error_ts("comptime_include!: expected \"name\" [, default]");
    }
    let path = PathBuf::from(get_env_var("CARGO_MANIFEST_DIR"))
        .join("comptime")
        .join(name);

    #[cfg(feature = "nightly")]
    proc_macro::tracked_path::path(&path);

    if let Ok(content) = fs::read_to_string(&path) {
        match content.parse::<TokenStream>() {
            Ok(ts) => ts,
            Err(_) => {
                compile_error_ts(&format!("comptime file '{}' contains invalid tokens", name))
            }
        }
    } else if !default.is_empty() {
        default
            .parse()
            .unwrap_or_else(|_| compile_error_ts(&format!("comptime default for '{}' is invalid", name)))
    } else {
        let msg = format!(
            "comptime file '{}' not found yet — run `cargo comptime` first, or provide a default: \
             call!(\"{}\", <default>) / call!(full, \"{}\", <default>)",
            name, name, name
        );
        if item_ctx {
            compile_error_ts(&msg)
        } else {
            format!("compile_error!({:?})", msg)
                .parse()
                .unwrap_or_default()
        }
    }
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

fn split_func_input(input: &str) -> Option<(String, String)> {
    let s = input.trim();
    let start = s.find('"')?;
    let rest = &s[start + 1..];
    let mut end = None;
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
            continue;
        }
        if c == '"' {
            end = Some(i);
            break;
        }
    }
    let end = end?;
    let name = rest[..end].to_string();
    let after = rest[end + 1..].trim();
    let args = match after.find(',') {
        Some(i) => after[i + 1..].trim().to_string(),
        None => String::new(),
    };
    Some((name, args))
}

fn has_comptime_attr(fn_node: Node, src: &[u8]) -> bool {
    let mut prev = fn_node.prev_sibling();
    while let Some(p) = prev {
        if p.kind() == "attribute_item" {
            if let Ok(t) = p.utf8_text(src) {
                if t.contains("comptime") {
                    return true;
                }
            }
        } else {
            break;
        }
        prev = p.prev_sibling();
    }
    false
}

fn find_fn_by_name(manifest_dir: &str, name: &str) -> Result<(PathBuf, String, String), String> {
    let base = PathBuf::from(manifest_dir);
    let mut stack = vec![base.join("src")];
    for extra in ["tests", "examples", "benches"] {
        let p = base.join(extra);
        if p.is_dir() {
            stack.push(p);
        }
    }
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file()
                    && path.extension().map_or(false, |e| e == "rs")
                    && !path.to_str().unwrap_or("").contains("_comptime.rs")
                {
                    let Ok(content) = fs::read_to_string(&path) else { continue };
                    let mut p = Parser::new();
                    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
                    let Some(tree) = p.parse(&content, None) else { continue };
                    let Ok(q) = Query::new(
                        &tree_sitter_rust::LANGUAGE.into(),
                        "(function_item name: (identifier) @n)",
                    ) else { continue };
                    let mut qc = QueryCursor::new();
                    let mut qm = qc.matches(&q, tree.root_node(), content.as_bytes());
                    while let Some(mm) = qm.next() {
                        for cap in mm.captures {
                            let Ok(n) = cap.node.utf8_text(content.as_bytes()) else { continue };
                            if n != name {
                                continue;
                            }
                            let Some(fn_node) = cap.node.parent() else { continue };
                            if !has_comptime_attr(fn_node, content.as_bytes()) {
                                continue;
                            }
                            if let Ok(item_text) = fn_node.utf8_text(content.as_bytes()) {
                                candidates.push((path.clone(), item_text.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }
    match candidates.first() {
        Some((p, item)) => {
            let source = fs::read_to_string(p).unwrap_or_default();
            Ok((p.clone(), source, item.clone()))
        }
        None => Err(format!(
            "func!: comptime fn '{}' not found in this crate (searched src/tests/examples/benches); \
             make sure it exists and is annotated #[comptime]",
            name
        )),
    }
}

fn token_tree_inner(body: &str, s: usize, e: usize) -> Result<String, String> {
    let text = &body[s..e];
    let open = text
        .find(['{', '(', '['])
        .ok_or_else(|| "func!: expected a block/group after `source!`".to_string())?;
    let inner_start = s + open + 1;
    let inner_end = e - 1;
    if inner_end <= inner_start {
        return Ok(String::new());
    }
    Ok(body[inner_start..inner_end].to_string())
}

fn output_parts(body: &str, s: usize, e: usize) -> Result<(String, String), String> {
    let text = &body[s..e];
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(text, None) else {
        return Err("func!: cannot parse output! invocation".to_string());
    };
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(macro_invocation macro: (identifier) @m (token_tree) @tt)",
    ) else {
        return Err("func!: internal query error".to_string());
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, tree.root_node(), text.as_bytes());
    let tt_node = qm.next().and_then(|mm| {
        mm.captures
            .iter()
            .find(|cap| q.capture_names()[cap.index as usize] == "tt")
            .map(|cap| cap.node)
    });
    let Some(tt_node) = tt_node else {
        return Err("func!: cannot locate output! arguments".to_string());
    };
    let mut children = Vec::new();
    {
        let mut c = tt_node.walk();
        for child in tt_node.children(&mut c) {
            children.push(child);
        }
    }
    let Some(first) = children.iter().find(|c| {
        c.utf8_text(text.as_bytes()).map_or(false, |t| {
            let t = t.trim();
            t == "raw" || t == "str"
        })
    }) else {
        return Err(
            "func!: output! must start with `raw` or `str`, e.g. output!(raw, res, \"hasil\")"
                .to_string(),
        );
    };
    let kind = first
        .utf8_text(text.as_bytes())
        .unwrap_or("")
        .trim()
        .to_string();
    if kind != "raw" && kind != "str" {
        return Err(format!(
            "func!: output! must start with `raw` or `str`, got `{}`",
            kind
        ));
    }
    let str_node = children
        .iter()
        .rev()
        .find(|c| c.kind() == "string_literal");
    let Some(str_node) = str_node else {
        return Err(
            "func!: output! needs a label string, e.g. output!(raw, res, \"hasil\")".to_string(),
        );
    };
    let expr_start = first.end_byte();
    let expr_end = str_node.start_byte();
    let expr = text[expr_start..expr_end]
        .trim()
        .trim_matches(',')
        .trim()
        .to_string();
    if expr.is_empty() {
        return Err("func!: output! has an empty value expression".to_string());
    }
    Ok((kind, expr))
}

fn transform_body(body: &str) -> Result<(String, usize), String> {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(body, None) else {
        return Err("func!: failed to parse comptime fn body".to_string());
    };
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(macro_invocation macro: (identifier) @m_name (_) @m_body)",
    ) else {
        return Err("func!: internal query error".to_string());
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, tree.root_node(), body.as_bytes());

    let mut collected: Vec<(usize, usize, String)> = Vec::new();
    while let Some(mm) = qm.next() {
        for cap in mm.captures {
            let cname = q.capture_names()[cap.index as usize];
            if cname == "m_name" {
                let name = cap.node.utf8_text(body.as_bytes()).unwrap_or("");
                if let Some(parent) = cap.node.parent() {
                    collected.push((parent.start_byte(), parent.end_byte(), name.to_string()));
                }
            }
        }
    }
    collected.sort_by_key(|(s, _, _)| *s);

    let mut out = String::new();
    let mut last = 0usize;
    let mut count = 0usize;
    let mut covered: Vec<(usize, usize)> = Vec::new();
    for (s, e, name) in collected {
        if covered.iter().any(|(cs, ce)| s >= *cs && e <= *ce) {
            continue;
        }
        out.push_str(&body[last..s]);
        match name.as_str() {
            "source" => {
                let inner = token_tree_inner(body, s, e)?;
                let (t, c) = transform_body(&inner)?;
                count += c;
                out.push_str("{\n");
                out.push_str(&t);
                out.push('\n');
                out.push('}');
            }
            "output" => {
                let (kind, expr) = output_parts(body, s, e)?;
                if kind == "str" {
                    out.push_str(&format!(
                        "{{ __fcomptime_val = Some(format!(\"{{}}\", {})); }}",
                        expr
                    ));
                } else {
                    out.push_str(&format!("{{ __fcomptime_val = Some({}); }}", expr));
                }
                count += 1;
            }
            "async_source" => {
                return Err(
                    "func!: async_source! is not supported inside func! inlining (use sync source!)"
                        .to_string(),
                );
            }
            _ => {
                out.push_str(&body[s..e]);
            }
        }
        last = e;
        covered.push((s, e));
    }
    out.push_str(&body[last..]);
    Ok((out, count))
}

#[proc_macro]
pub fn func(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();
    let (name, args_text) = match split_func_input(&input_str) {
        Some(v) => v,
        None => {
            return compile_error_ts(
                "func!: expected `func!(\"fn_name\", arg1, arg2, ...)` with the fn name as a \
                 string literal",
            );
        }
    };
    if name.is_empty() {
        return compile_error_ts(
            "func!: fn name must be a string literal, e.g. func!(\"math\", 10)",
        );
    }

    let manifest_dir = get_env_var("CARGO_MANIFEST_DIR");

    let (file_path, file_source, item_text) = match find_fn_by_name(&manifest_dir, &name) {
        Ok(v) => v,
        Err(e) => return compile_error_ts(&e),
    };

    #[cfg(feature = "nightly")]
    proc_macro::tracked_path::path(&file_path);

    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(item_tree) = p.parse(&item_text, None) else {
        return compile_error_ts(&format!("func!: failed to parse fn '{}'", name));
    };
    let item_root = item_tree.root_node();

    let (all_params, bindable) = fn_params_and_bindable(&item_text, item_root);

    if all_params.iter().any(|p| p == "self") {
        return compile_error_ts(&format!(
            "func!: comptime fn '{}' has a `self` parameter; func! only supports plain value \
             parameters",
            name
        ));
    }
    if all_params.len() != bindable.len() {
        return compile_error_ts(&format!(
            "func!: comptime fn '{}' uses a destructured parameter pattern; only plain \
             `name: type` parameters are supported",
            name
        ));
    }

    let args: Vec<String> = split_top_level(&args_text, ',')
        .into_iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();

    if args.len() != bindable.len() {
        return compile_error_ts(&format!(
            "func!: comptime fn '{}' takes {} parameter(s) ({}) but {} argument(s) were given",
            name,
            bindable.len(),
            if bindable.is_empty() {
                "none".to_string()
            } else {
                bindable.join(", ")
            },
            args.len()
        ));
    }

    let Ok(q_body) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(function_item body: (block) @b)",
    ) else {
        return compile_error_ts("func!: internal query error");
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q_body, item_root, item_text.as_bytes());
    let body_text = qm.next().and_then(|mm| {
        mm.captures.first().and_then(|cap| {
            cap.node
                .utf8_text(item_text.as_bytes())
                .ok()
                .map(|t| t.to_string())
        })
    });
    let Some(body_text) = body_text else {
        return compile_error_ts(&format!("func!: cannot locate the body of fn '{}'", name));
    };

    if has_self_in_str(&body_text) {
        return compile_error_ts(&format!(
            "func!: comptime fn '{}' references `self` in its body; extract the needed fields \
             into plain values before source!",
            name
        ));
    }

    let (transformed, output_count) = match transform_body(&body_text) {
        Ok(v) => v,
        Err(e) => return compile_error_ts(&e),
    };
    if output_count == 0 {
        return compile_error_ts(&format!(
            "func!: comptime fn '{}' never calls output!(raw|str, ...) inside its source!, so \
             func! has no value to produce; add an output!(raw, <value>, \"label\") call",
            name
        ));
    }

    let panic_msg = format!(
        "func!: comptime fn '{}' executed but no output!(raw|str, ...) ran along the taken path",
        name
    );

    let rerun = build_rerun_token(&file_path, &file_source);

    let mut code = format!("{{ {} ", rerun);
    code.push_str("let mut __fcomptime_val: Option<_> = None;\n    {\n");
    for (param, arg) in bindable.iter().zip(args.iter()) {
        code.push_str(&format!("        let {} = {};\n", param, arg));
    }
    for line in transformed.lines() {
        code.push_str("        ");
        code.push_str(line);
        code.push('\n');
    }
    code.push_str("    }\n");
    code.push_str("    match __fcomptime_val {\n");
    code.push_str("        Some(__fcomptime_v) => __fcomptime_v,\n");
    code.push_str(&format!("        None => panic!({:?}),\n", panic_msg));
    code.push_str("    }\n}");

    match code.parse::<TokenStream>() {
        Ok(ts) => ts,
        Err(e) => compile_error_ts(&format!("func!: internal expansion error: {}", e)),
    }
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

    #[test]
    fn split_top_level_respects_brackets_and_strings() {
        let parts = split_top_level("a: 5, b: vec![1, 2], c: f(\"x, y\")", ',');
        assert_eq!(parts, vec!["a: 5", " b: vec![1, 2]", " c: f(\"x, y\")"]);
        let parts = split_top_level("a: vec![1,2]", ',');
        assert_eq!(parts.len(), 1);
        let parts = split_top_level("f(x: 1, y: 2)", ':');
        assert_eq!(parts.len(), 1);
        let parts = split_top_level("a: f(x: 1, y: 2)", ':');
        assert_eq!(parts, vec!["a", " f(x: 1, y: 2)"]);
        let parts = split_top_level("a:5", ':');
        assert_eq!(parts, vec!["a", "5"]);
    }

    #[test]
    fn has_self_detection() {
        assert!(has_self_in_str("let y = self.x + 1;"));
        assert!(has_self_in_str("self.method();"));
        assert!(!has_self_in_str("let s = 5; selfish();"));
    }

    #[test]
    fn compile_error_string_emits_message() {
        let s = compile_error_string("hello world");
        assert!(s.contains("compile_error!"));
        assert!(s.contains("hello world"));
    }

    #[test]
    fn split_func_input_parses_name_and_args() {
        assert_eq!(
            split_func_input("\"math\", 10"),
            Some(("math".to_string(), "10".to_string()))
        );
        assert_eq!(
            split_func_input("\"area\", 3, 4"),
            Some(("area".to_string(), "3, 4".to_string()))
        );
        assert_eq!(
            split_func_input("\"noargs\""),
            Some(("noargs".to_string(), String::new()))
        );
        assert_eq!(
            split_func_input("\"math\", v * 2 + 1"),
            Some(("math".to_string(), "v * 2 + 1".to_string()))
        );
        assert_eq!(
            split_func_input("\"math\", vec![1, 2]"),
            Some(("math".to_string(), "vec![1, 2]".to_string()))
        );
        assert_eq!(split_func_input("math"), None);
    }

    #[test]
    fn transform_body_unwraps_source_and_rewrites_outputs() {
        let body = "{\n    source! {\n        let res = i * 2;\n        output!(raw, res, \"hasil\");\n    }\n}";
        let (out, count) = transform_body(body).unwrap();
        assert_eq!(count, 1);
        assert!(out.contains("let res = i * 2;"));
        assert!(out.contains("__fcomptime_val = Some(res);"));
        assert!(!out.contains("output!"));
        assert!(!out.contains("source!"));
    }

    #[test]
    fn transform_body_handles_branches_and_multiple_outputs() {
        let body = "{\n    if i > 5 {\n        output!(raw, i, \"a\");\n    } else {\n        output!(raw, i * 3, \"b\");\n    }\n}";
        let (out, count) = transform_body(body).unwrap();
        assert_eq!(count, 2);
        assert!(out.contains("Some(i);"));
        assert!(out.contains("Some(i * 3);"));
    }

    #[test]
    fn transform_body_keeps_other_macros() {
        let body = "{\n    let x = 1;\n    call_scope! {\n        let y = 2;\n    }\n    println!(\"x\");\n}";
        let (out, count) = transform_body(body).unwrap();
        assert_eq!(count, 0);
        assert!(out.contains("call_scope!"));
        assert!(out.contains("println!"));
        assert!(out.contains("let x = 1;"));
    }

    #[test]
    fn transform_body_rejects_async_source() {
        let body = "{\n    async_source! {\n        let v = 1;\n    }\n}";
        let err = transform_body(body);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("async_source"));
    }

    #[test]
    fn output_parts_extracts_raw_and_str_values() {
        let text = "output!(raw, a + b, \"label\")";
        let s = text.find("output").unwrap();
        let e = text.len();
        let (kind, expr) = output_parts(text, s, e).unwrap();
        assert_eq!(kind, "raw");
        assert_eq!(expr, "a + b");

        let text2 = "output!(str, format!(\"x: {}\", n), \"label\")";
        let s2 = text2.find("output").unwrap();
        let (kind2, expr2) = output_parts(text2, s2, text2.len()).unwrap();
        assert_eq!(kind2, "str");
        assert_eq!(expr2, "format!(\"x: {}\", n)");

        let text3 = "output!(raw, vec![1, 2], \"l\")";
        let s3 = text3.find("output").unwrap();
        let (_, expr3) = output_parts(text3, s3, text3.len()).unwrap();
        assert_eq!(expr3, "vec![1, 2]");
    }

    #[test]
    fn token_tree_inner_extracts_block_content() {
        let body = "source! { let a = 1; }";
        let (inner, count) = transform_body(body).unwrap();
        assert_eq!(count, 0);
        assert!(inner.contains("let a = 1;"));
    }
}
