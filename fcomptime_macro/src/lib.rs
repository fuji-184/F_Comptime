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
    let item_root = item_tree.root_node();

    let has_kind = |kind: &str| -> bool {
        let Ok(q) = Query::new(&tree_sitter_rust::LANGUAGE.into(), &format!("({}) @i", kind))
        else {
            return false;
        };
        let mut c = QueryCursor::new();
        let mut m = c.matches(&q, item_root, item_str.as_bytes());
        m.next().is_some()
    };

    let root_kind = if has_kind("trait_item") {
        "trait_item"
    } else if has_kind("impl_item") {
        "impl_item"
    } else if has_kind("function_item") {
        "function_item"
    } else {
        "other"
    };

    let mut test_mods = String::new();
    let mut errors: Vec<String> = Vec::new();
    let mut is_method = false;
    let mut rewritten_item: Option<String> = None;

    match root_kind {
        "function_item" => {
            let item_fn_name = {
                let q = Query::new(
                    &tree_sitter_rust::LANGUAGE.into(),
                    "(function_item name: (identifier) @n)",
                );
                match q {
                    Ok(q) => {
                        let mut c = QueryCursor::new();
                        let mut m = c.matches(&q, item_root, item_str.as_bytes());
                        m.next().and_then(|mm| mm.captures.first())
                            .map(|cap| cap.node.utf8_text(item_str.as_bytes()).unwrap_or("").to_string())
                            .unwrap_or_default()
                    }
                    Err(_) => String::new(),
                }
            };

            is_method = {
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

            if is_method {
                errors.push(
                    "comptime: #[comptime] on an impl method is not supported directly (the \
                     generated test module cannot be placed inside an impl block); put #[comptime] \
                     on the enclosing impl block instead, e.g. `#[comptime] impl Foo { ... }`, and \
                     keep the method free of value parameters (only `self` is allowed)"
                        .to_string(),
                );
            } else {
                let (mods, errs, rewritten) = build_test_for_fn(
                    &item_str,
                    base_line + 1,
                    &source_code,
                    root_node,
                    &item_fn_name,
                    ParamPolicy::Skip,
                    &[],
                    false,
                );
                test_mods.push_str(&mods);
                errors.extend(errs);
                rewritten_item = rewritten;
            }
        }
        "impl_item" | "trait_item" => {
            if let Some(container_node) =
                find_kind_node(item_root, item_str.as_bytes(), root_kind)
            {
                let start_row =
                    find_container_start_row(root_node, &source_code, base_line, root_kind);
                let generics = container_generics(&item_str, container_node);
                let mut item_edits: Vec<(usize, usize, String)> = Vec::new();
                for method in direct_functions(container_node) {
                    let Ok(method_text) = method.utf8_text(item_str.as_bytes()) else {
                        continue;
                    };
                    let method_name = fn_name_of(method, item_str.as_bytes());
                    let method_line = start_row
                        .map(|r| r + method.start_position().row + 1)
                        .unwrap_or(base_line + 2 + method.start_position().row);
                    let (mods, errs, rewritten) = build_test_for_fn(
                        &method_text.to_string(),
                        method_line,
                        &source_code,
                        root_node,
                        &method_name,
                        ParamPolicy::ErrorIfNonSelf,
                        &generics,
                        true,
                    );
                    test_mods.push_str(&mods);
                    errors.extend(errs);
                    if let Some(r) = rewritten {
                        item_edits.push((method.start_byte(), method.end_byte(), r));
                    }
                }
                if !item_edits.is_empty() {
                    item_edits.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
                    let mut out = item_str.clone();
                    for (s, e, r) in item_edits {
                        out.replace_range(s..e, &r);
                    }
                    rewritten_item = Some(out);
                }
            }
        }
        _ => {}
    }

    let mut output = if is_method {
        TokenStream::new()
    } else {
        rerun_token
    };
    match rewritten_item {
        Some(text) => output.extend(text.parse::<TokenStream>().unwrap_or_else(|_| item)),
        None => output.extend(item),
    }
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

#[derive(Clone, Copy, PartialEq)]
enum ParamPolicy {
    Skip,
    ErrorIfNonSelf,
}

fn find_kind_node<'a>(root: Node<'a>, src: &'a [u8], kind: &str) -> Option<Node<'a>> {
    let Ok(q) = Query::new(&tree_sitter_rust::LANGUAGE.into(), &format!("({}) @i", kind)) else {
        return None;
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, root, src);
    qm.next().and_then(|m| m.captures.first()).map(|cap| cap.node)
}

fn direct_functions(container_root: Node) -> Vec<Node> {
    let mut out = Vec::new();
    let mut c = container_root.walk();
    for child in container_root.children(&mut c) {
        if child.kind() != "declaration_list" {
            continue;
        }
        let mut cc = child.walk();
        for item in child.children(&mut cc) {
            if item.kind() == "function_item" {
                out.push(item);
            }
        }
    }
    out
}

fn fn_name_of(node: Node, src: &[u8]) -> String {
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(src).ok())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn container_generics(item_str: &str, container_root: Node) -> Vec<String> {
    let mut out = Vec::new();
    let Some(tp) = container_root.child_by_field_name("type_parameters") else {
        return out;
    };
    let mut c = tp.walk();
    for child in tp.children(&mut c) {
        if child.kind() != "type_parameter" && child.kind() != "constrained_type_parameter" {
            continue;
        }
        if let Some(id) = child.child_by_field_name("name") {
            if let Ok(t) = id.utf8_text(item_str.as_bytes()) {
                let t = t.trim().to_string();
                if !t.is_empty() && !out.contains(&t) {
                    out.push(t);
                }
            }
        }
    }
    out
}

fn find_container_start_row(
    file_root: Node,
    source_code: &str,
    base_line: usize,
    kind: &str,
) -> Option<usize> {
    let Ok(q) = Query::new(&tree_sitter_rust::LANGUAGE.into(), &format!("({}) @c", kind)) else {
        return None;
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, file_root, source_code.as_bytes());
    let mut best: Option<(usize, usize)> = None;
    while let Some(mm) = qm.next() {
        for cap in mm.captures {
            let line = cap.node.start_position().row + 1;
            if line.abs_diff(base_line) > 3 {
                continue;
            }
            let dist = line.abs_diff(base_line);
            let better = match best {
                Some((bd, bl)) => dist < bd || (dist == bd && line > bl),
                None => true,
            };
            if better {
                best = Some((dist, cap.node.start_position().row));
            }
        }
    }
    best.map(|(_, row)| row)
}

fn build_test_for_fn(
    item_str: &str,
    fn_line: usize,
    source_code: &str,
    root_node: Node,
    fn_name: &str,
    policy: ParamPolicy,
    container_generics: &[String],
    check_self_type: bool,
) -> (String, Vec<String>, Option<String>) {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(item_tree) = parser.parse(item_str, None) else {
        return (String::new(), Vec::new(), None);
    };
    let item_root = item_tree.root_node();

    let (fn_params, _fn_bindable) = fn_params_and_bindable(item_str, item_root);

    let body_block = {
        let q = Query::new(
            &tree_sitter_rust::LANGUAGE.into(),
            "(function_item body: (block) @b)",
        );
        match q {
            Ok(q) => {
                let mut c = QueryCursor::new();
                let mut m = c.matches(&q, item_root, item_str.as_bytes());
                m.next().and_then(|mm| mm.captures.first()).map(|cap| cap.node)
            }
            Err(_) => None,
        }
    };

    let Some(body_block) = body_block else {
        return (String::new(), Vec::new(), None);
    };

    let mut all_generics: Vec<String> = container_generics.to_vec();
    if let Some(tp) = item_root.child_by_field_name("type_parameters") {
        let mut c = tp.walk();
        for child in tp.children(&mut c) {
            if child.kind() != "type_parameter" && child.kind() != "constrained_type_parameter" {
                continue;
            }
            if let Some(id) = child.child_by_field_name("name") {
                if let Ok(t) = id.utf8_text(item_str.as_bytes()) {
                    let t = t.trim().to_string();
                    if !t.is_empty() && !all_generics.contains(&t) {
                        all_generics.push(t);
                    }
                }
            }
        }
    }

    let query_macro = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(macro_invocation macro: (identifier) @m_name (_) @m_body)",
    ).unwrap();

    let mut macro_cursor = QueryCursor::new();
    let mut macro_matches =
        macro_cursor.matches(&query_macro, item_root, item_str.as_bytes());

    let mut test_mods = String::new();
    let mut errors: Vec<String> = Vec::new();
    let mut param_err_pushed = false;
    let mut item_edits: Vec<(usize, usize, String)> = Vec::new();

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

        if macro_name != "source"
            && macro_name != "async_source"
            && macro_name != "call_scope"
        {
            continue;
        }

        match policy {
            ParamPolicy::Skip => {
                if !fn_params.is_empty() {
                    continue;
                }
            }
            ParamPolicy::ErrorIfNonSelf => {
                if fn_params.iter().any(|p| p != "self") {
                    if !param_err_pushed {
                        errors.push(format!(
                            "comptime: method '{}' has value parameters (only `self` is allowed); \
                             the generated test cannot capture them — extract the computation into \
                             a free #[comptime] fn called with func!, or remove the parameters",
                            fn_name
                        ));
                        param_err_pushed = true;
                    }
                    continue;
                }
            }
        }

        let body_node = match body_node {
            Some(b) => b,
            None => continue,
        };

        let m_node = macro_node.unwrap();

        if macro_name == "call_scope" && is_inside_framework_macro(m_node, item_str.as_bytes()) {
            errors.push(format!(
                "comptime error (line {}): call_scope! nested inside source!/async_source!/call_scope! \
                 is not supported",
                fn_line + m_node.start_position().row
            ));
            continue;
        }

        let macro_relative_row = m_node.start_position().row;

        let expected_line = fn_line + macro_relative_row;
        let mut call_line = expected_line;
        let mut best_dist = usize::MAX;

        for (i, line) in source_code
            .lines()
            .enumerate()
            .skip(fn_line.saturating_sub(1))
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

        let item_call_byte = {
            let mut cur = m_node;
            loop {
                match cur.parent() {
                    Some(p) if p.id() == body_block.id() => break Some((cur.start_byte(), cur.end_byte())),
                    Some(p) => cur = p,
                    None => break None,
                }
            }
        };

        let mut call_errors: Vec<String> = Vec::new();

        let macro_label = format!("{}!", macro_name);

        if let Err(e) = check_source_at_fn_top(m_node, body_block, macro_name) {
            call_errors.push(e);
        }

        let prefix_text = fn_prefix_text(item_str, Some(body_block), item_call_byte.map(|(s, _)| s));
        let prefix_bound = bound_names_in_str(&prefix_text);

        if has_self_in_str(&body_only) || has_self_in_str(&prefix_text) {
            if check_self_type {
                call_errors.push(format!(
                    "comptime error (line {}): {} body or captured prefix references \
                     `self`; instance state cannot be captured at compile time — comptime code \
                     of an impl method must not depend on `self`, use module/associated data \
                     instead, or extract the computation into a free #[comptime] fn called with \
                     func!",
                    call_line, macro_label
                ));
            } else {
                call_errors.push(format!(
                    "comptime error (line {}): {} body or captured prefix references `self`; \
                     `self` cannot be captured at compile time — extract the needed fields into \
                     locals before {}, e.g. `let x = self.x;`",
                    call_line, macro_label, macro_label
                ));
            }
        }

        if check_self_type {
            if has_self_type_in_str(&body_only) || has_self_type_in_str(&prefix_text) {
                call_errors.push(format!(
                    "comptime error (line {}): {} body or captured prefix references `Self`; \
                     `Self` cannot be resolved inside the generated test — use the concrete type \
                     name instead, or extract the values into locals before {}",
                    call_line, macro_label, macro_label
                ));
            }
        }

        if !all_generics.is_empty() {
            let code = format!("{}\n{}", prefix_text, body_only);
            let idents = identifiers_in_str(&code);
            let bound = bound_names_in_str(&code);
            for g in &all_generics {
                if idents.iter().any(|i| i == g) && !bound.contains(g) {
                    call_errors.push(format!(
                        "comptime error (line {}): {} body or captured prefix references \
                         generic parameter `{}` of the enclosing item; the generated test cannot \
                         resolve it",
                        call_line, macro_label, g
                    ));
                    break;
                }
            }
        }

        let after_names = names_bound_after(item_str, Some(body_block), item_call_byte.map(|(_, e)| e));
        for t in &targets {
            if after_names.contains(t) && !prefix_bound.contains(t) {
                call_errors.push(format!(
                    "comptime error (line {}): {} body references `{}` which is defined after \
                     the {} call; move its definition before {} so it can be captured",
                    call_line, macro_label, t, macro_label, macro_label
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

        if macro_name == "call_scope" {
            let (fco_content, fco_refs) = match build_fco_test_content(&body_only, call_line) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            let fco_prefix = filter_prefix_for_fco(&prefix_text, &fco_refs);
            let mut setup = String::new();
            if !module_consts.is_empty() {
                for c in &module_consts {
                    setup.push_str("        ");
                    setup.push_str(c.trim());
                    setup.push('\n');
                }
            }
            setup.push_str(&fco_prefix);

            let test_fn_name = format!("comptime_fco_{}_{}", call_line, macro_relative_row);
            let test_mod = format!(
                "#[cfg(test)]\nmod {} {{\n    use super::*;\n    #[allow(unused_variables, unused_assignments)]\n    #[test]\n    fn run() {{\n{}{}}}\n}}\n",
                test_fn_name, setup, fco_content,
            );
            test_mods.push_str(&test_mod);

            if let Some(inv) = m_node.parent() {
                if inv.kind() == "macro_invocation" {
                    if let Ok(inv_text) = inv.utf8_text(item_str.as_bytes()) {
                        match label_func_calls_in_text(inv_text, call_line) {
                            Ok(labeled) if labeled != inv_text => {
                                item_edits.push((inv.start_byte(), inv.end_byte(), labeled));
                            }
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }

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

    if item_edits.is_empty() {
        (test_mods, errors, None)
    } else {
        item_edits.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
        let mut out = item_str.to_string();
        for (s, e, r) in item_edits {
            out.replace_range(s..e, &r);
        }
        (test_mods, errors, Some(out))
    }
}

fn has_self_type_in_str(text: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|t| t == "Self")
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

fn check_source_at_fn_top(m_node: Node, body_block: Node, macro_name: &str) -> Result<(), String> {
    let mut cur = m_node.parent();
    while let Some(p) = cur {
        if p.kind() == "block" {
            if p.id() != body_block.id() {
                return Err(format!(
                    "comptime error: {}! must be called at the top level of the #[comptime] \
                     fn body (not inside if/loop/nested block), because values there may depend \
                     on runtime control flow",
                    macro_name
                ));
            }
            return Ok(());
        }
        cur = p.parent();
    }
    Err(format!(
        "comptime error: cannot locate {}! within the #[comptime] fn body",
        macro_name
    ))
}

fn is_inside_framework_macro(node: Node, src: &[u8]) -> bool {
    let mut cur = node.parent().and_then(|p| p.parent());
    while let Some(p) = cur {
        if p.kind() == "macro_invocation" {
            if let Some(name_node) = p.child_by_field_name("macro") {
                if let Ok(n) = name_node.utf8_text(src) {
                    if n == "source" || n == "async_source" || n == "call_scope" {
                        return true;
                    }
                }
            }
        }
        cur = p.parent();
    }
    false
}

fn macro_invocations_in(text: &str) -> Vec<(usize, usize, String, usize, usize)> {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(text, None) else {
        return Vec::new();
    };
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(macro_invocation macro: (identifier) @m_name (_) @m_body)",
    ) else {
        return Vec::new();
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, tree.root_node(), text.as_bytes());
    let mut out = Vec::new();
    while let Some(mm) = qm.next() {
        let mut name = String::new();
        let mut inv: Option<(usize, usize)> = None;
        let mut tt: Option<(usize, usize)> = None;
        for cap in mm.captures {
            match q.capture_names()[cap.index as usize] {
                "m_name" => {
                    name = cap.node.utf8_text(text.as_bytes()).unwrap_or("").to_string();
                    inv = cap.node.parent().map(|p| (p.start_byte(), p.end_byte()));
                }
                "m_body" => tt = Some((cap.node.start_byte(), cap.node.end_byte())),
                _ => {}
            }
        }
        if let (Some((is, ie)), Some((ts, te))) = (inv, tt) {
            out.push((is, ie, name, ts, te));
        }
    }
    out
}

fn call_rewrite_info(
    inv_start: usize,
    inv_end: usize,
    tt_start: usize,
    tt_end: usize,
    text: &str,
) -> Option<(usize, usize, String)> {
    let tt_text = &text[tt_start..tt_end];
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(tt_text, None) else {
        return None;
    };
    let root = tree.root_node();
    let mut children: Vec<Node> = Vec::new();
    {
        let mut c = root.walk();
        for child in root.children(&mut c) {
            children.push(child);
        }
        while children.len() == 1
            && children[0].kind() != "string_literal"
            && children[0].kind() != "identifier"
        {
            let mut c2 = children[0].walk();
            let kids: Vec<Node> = children[0].children(&mut c2).collect();
            children = kids
                .into_iter()
                .filter(|n| n.kind() != "(" && n.kind() != ")" && n.kind() != "," && n.kind() != ";")
                .collect();
            if children.is_empty() {
                break;
            }
        }
    }
    let get_name = |node: Node| -> Option<String> {
        node.utf8_text(tt_text.as_bytes())
            .ok()
            .map(|s| s.trim().trim_matches('"').trim().to_string())
            .filter(|s| !s.is_empty())
    };
    match children.first() {
        Some(first) if first.kind() == "string_literal" => {
            let name = get_name(*first)?;
            Some((inv_start, inv_end, name))
        }
        Some(first) if first.kind() == "identifier" => {
            let f = first.utf8_text(tt_text.as_bytes()).unwrap_or("");
            if f == "token" {
                let second = children.get(1)?;
                if second.kind() != "string_literal" {
                    return None;
                }
                let name = get_name(*second)?;
                Some((inv_start, inv_end, name))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn stmt_macro_names(stmt: Node, src: &[u8]) -> Vec<String> {
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(macro_invocation macro: (identifier) @m)",
    ) else {
        return Vec::new();
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, stmt, src);
    let mut out = Vec::new();
    while let Some(mm) = qm.next() {
        for cap in mm.captures {
            if let Ok(n) = cap.node.utf8_text(src) {
                out.push(n.to_string());
            }
        }
    }
    out
}

fn stmt_bound_names(stmt: Node, src: &[u8]) -> Vec<String> {
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "[(let_declaration pattern: (_) @p) (assignment_expression left: (_) @l) \
         (compound_assignment_expr left: (_) @l)]",
    ) else {
        return Vec::new();
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, stmt, src);
    let mut out = Vec::new();
    while let Some(mm) = qm.next() {
        for cap in mm.captures {
            if cap.node.kind() == "identifier" {
                if let Ok(t) = cap.node.utf8_text(src) {
                    if !out.contains(&t.to_string()) {
                        out.push(t.to_string());
                    }
                }
            } else {
                let mut ids = Vec::new();
                collect_identifiers(cap.node, src, &mut ids);
                for i in ids {
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
            }
        }
    }
    out
}

fn stmt_referenced_idents(stmt: Node, src: &[u8]) -> HashSet<String> {
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "(identifier) @name",
    ) else {
        return HashSet::new();
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, stmt, src);
    let mut out = HashSet::new();
    while let Some(mm) = qm.next() {
        for cap in mm.captures {
            let node = cap.node;
            let mut skip = false;
            if let Some(par) = node.parent() {
                match par.kind() {
                    "macro_invocation" => skip = true,
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
                    _ => {}
                }
            }
            if skip {
                continue;
            }
            if let Ok(t) = node.utf8_text(src) {
                let t = t.to_string();
                if !is_framework_name(&t) {
                    out.insert(t);
                }
            }
        }
    }
    out
}

fn block_statement_nodes<'a>(root: Node<'a>, src: &'a [u8]) -> Vec<Node<'a>> {
    let Ok(q) = Query::new(&tree_sitter_rust::LANGUAGE.into(), "(block) @b") else {
        return Vec::new();
    };
    let mut qc = QueryCursor::new();
    let mut qm = qc.matches(&q, root, src);
    let Some(block) = qm.next().and_then(|mm| mm.captures.first()).map(|cap| cap.node) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut c = block.walk();
    for child in block.children(&mut c) {
        let k = child.kind();
        if k == "{" || k == "}" || k == ";" {
            continue;
        }
        out.push(child);
    }
    out
}

fn label_func_calls_in_inner(inner: &str, call_line: usize) -> Result<String, String> {
    let wrapped = format!("{{\n{}\n}}", inner);
    let macros = macro_invocations_in(&wrapped);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut func_idx = 0usize;
    for (_, _, name, _, te) in &macros {
        if name != "func" {
            continue;
        }
        let label = format!("fco_{}_{}", call_line, func_idx);
        func_idx += 1;
        if *te > 2 && wrapped.as_bytes()[*te - 1] == b')' {
            edits.push((*te - 3, *te - 3, format!(", comptime_label = {:?}", label)));
        }
    }
    if edits.is_empty() {
        return Ok(inner.to_string());
    }
    edits.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
    let mut out = inner.to_string();
    for (s, e, r) in edits {
        out.replace_range(s..e, &r);
    }
    Ok(out)
}

fn label_func_calls_in_text(text: &str, call_line: usize) -> Result<String, String> {
    let (prefix, inner, suffix) = match (text.find('{'), text.rfind('}')) {
        (Some(open), Some(close)) if close > open => {
            (&text[..open + 1], &text[open + 1..close], &text[close..])
        }
        _ => ("", text, ""),
    };
    let labeled = label_func_calls_in_inner(inner, call_line)?;
    if labeled == inner {
        Ok(text.to_string())
    } else {
        Ok(format!("{}{}{}", prefix, labeled, suffix))
    }
}

fn build_fco_test_content(body_only: &str, call_line: usize) -> Result<(String, HashSet<String>), String> {
    let wrapped = format!("{{\n{}\n}}", body_only);
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(&wrapped, None) else {
        return Err("comptime error: cannot parse call_scope! content".to_string());
    };
    let root = tree.root_node();
    let stmts = block_statement_nodes(root, wrapped.as_bytes());

    let n = stmts.len();
    let mut has_func = vec![false; n];
    let mut bounds: Vec<HashSet<String>> = vec![HashSet::new(); n];
    let mut refs: Vec<HashSet<String>> = vec![HashSet::new(); n];
    for (i, stmt) in stmts.iter().enumerate() {
        has_func[i] = stmt_macro_names(*stmt, wrapped.as_bytes())
            .iter()
            .any(|m| m == "func");
        bounds[i] = stmt_bound_names(*stmt, wrapped.as_bytes()).into_iter().collect();
        let r = stmt_referenced_idents(*stmt, wrapped.as_bytes());
        refs[i] = r.difference(&bounds[i]).cloned().collect();
    }

    let mut kept = has_func.clone();
    loop {
        let mut referenced: HashSet<String> = HashSet::new();
        for i in 0..n {
            if kept[i] {
                referenced.extend(refs[i].iter().cloned());
            }
        }
        let mut changed = false;
        for i in 0..n {
            if kept[i] {
                continue;
            }
            if bounds[i].iter().any(|b| referenced.contains(b)) {
                kept[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut referenced_final: HashSet<String> = HashSet::new();
    for i in 0..n {
        if kept[i] {
            referenced_final.extend(refs[i].iter().cloned());
        }
    }

    let macros = macro_invocations_in(&wrapped);
    let mut func_edits: Vec<(usize, usize, String)> = Vec::new();
    let mut call_edits: Vec<(usize, usize, String)> = Vec::new();
    let mut func_idx = 0usize;
    for (is, ie, name, ts, te) in &macros {
        match name.as_str() {
            "func" => {
                let label = format!("fco_{}_{}", call_line, func_idx);
                func_idx += 1;
                if *te > 0 && wrapped.as_bytes()[*te - 1] == b')' {
                    func_edits.push((*te - 1, *te - 1, format!(", comptime_label = {:?}", label)));
                }
            }
            "call" => {
                if let Some((cs, ce, cname)) = call_rewrite_info(*is, *ie, *ts, *te, &wrapped) {
                    call_edits.push((cs, ce, format!("fcomptime::runtime_read!({:?})", cname)));
                }
            }
            _ => {}
        }
    }

    let mut out = String::new();
    for (i, stmt) in stmts.iter().enumerate() {
        if !kept[i] {
            continue;
        }
        let ss = stmt.start_byte();
        let se = stmt.end_byte();
        let mut local: Vec<(usize, usize, String)> = func_edits
            .iter()
            .chain(call_edits.iter())
            .filter(|(s, e, _)| *s >= ss && *e <= se)
            .map(|(s, e, r)| (s - ss, e - ss, r.clone()))
            .collect();
        local.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
        let mut text = stmt.utf8_text(wrapped.as_bytes()).unwrap_or("").to_string();
        for (s, e, r) in local {
            text.replace_range(s..e, &r);
        }
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") || t.starts_with("/*") {
                continue;
            }
            out.push_str("        ");
            out.push_str(t);
            out.push('\n');
        }
    }
    Ok((out, referenced_final))
}

fn filter_prefix_for_fco(prefix_text: &str, seed_refs: &HashSet<String>) -> String {
    if prefix_text.trim().is_empty() {
        return String::new();
    }
    let wrapped = format!("{{\n{}\n}}", prefix_text.trim());
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let Some(tree) = p.parse(&wrapped, None) else {
        return String::new();
    };
    let root = tree.root_node();
    let stmts = block_statement_nodes(root, wrapped.as_bytes());

    let n = stmts.len();
    let mut bounds: Vec<HashSet<String>> = vec![HashSet::new(); n];
    let mut refs: Vec<HashSet<String>> = vec![HashSet::new(); n];
    for (i, stmt) in stmts.iter().enumerate() {
        bounds[i] = stmt_bound_names(*stmt, wrapped.as_bytes()).into_iter().collect();
        let r = stmt_referenced_idents(*stmt, wrapped.as_bytes());
        refs[i] = r.difference(&bounds[i]).cloned().collect();
    }

    let mut kept = vec![false; n];
    loop {
        let mut referenced: HashSet<String> = seed_refs.clone();
        for i in 0..n {
            if kept[i] {
                referenced.extend(refs[i].iter().cloned());
            }
        }
        let mut changed = false;
        for i in 0..n {
            if kept[i] {
                continue;
            }
            if bounds[i].iter().any(|b| referenced.contains(b)) {
                kept[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let macros = macro_invocations_in(&wrapped);
    let mut call_edits: Vec<(usize, usize, String)> = Vec::new();
    for (is, ie, name, ts, te) in &macros {
        if name == "call" {
            if let Some((cs, ce, cname)) = call_rewrite_info(*is, *ie, *ts, *te, &wrapped) {
                call_edits.push((cs, ce, format!("fcomptime::runtime_read!({:?})", cname)));
            }
        }
    }

    let mut out = String::new();
    for (i, stmt) in stmts.iter().enumerate() {
        if !kept[i] {
            continue;
        }
        let ss = stmt.start_byte();
        let se = stmt.end_byte();
        let mut local: Vec<(usize, usize, String)> = call_edits
            .iter()
            .filter(|(s, e, _)| *s >= ss && *e <= se)
            .map(|(s, e, r)| (s - ss, e - ss, r.clone()))
            .collect();
        local.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
        let mut text = stmt.utf8_text(wrapped.as_bytes()).unwrap_or("").to_string();
        for (s, e, r) in local {
            text.replace_range(s..e, &r);
        }
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") || t.starts_with("/*") {
                continue;
            }
            out.push_str("        ");
            out.push_str(t);
            out.push('\n');
        }
    }
    out
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
    let Ok(q) = Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        "[(identifier) (type_identifier)] @name",
    ) else {
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

fn transform_body(body: &str) -> Result<(String, usize, String), String> {
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
    let mut last_kind = "raw".to_string();
    let mut covered: Vec<(usize, usize)> = Vec::new();
    for (s, e, name) in collected {
        if covered.iter().any(|(cs, ce)| s >= *cs && e <= *ce) {
            continue;
        }
        out.push_str(&body[last..s]);
        match name.as_str() {
            "source" => {
                let inner = token_tree_inner(body, s, e)?;
                let (t, c, k) = transform_body(&inner)?;
                count += c;
                if !k.is_empty() {
                    last_kind = k;
                }
                out.push_str("{\n");
                out.push_str(&t);
                out.push('\n');
                out.push('}');
            }
            "output" => {
                let (kind, expr) = output_parts(body, s, e)?;
                last_kind = kind.clone();
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
    Ok((out, count, last_kind))
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

    let mut comptime_label: Option<String> = None;
    let mut args = args;
    if let Some(last) = args.last() {
        if let Some(eq) = last.find('=') {
            if last[..eq].trim() == "comptime_label" {
                let rhs = last[eq + 1..].trim();
                let label = rhs.trim_matches('"').trim().to_string();
                if label.is_empty() {
                    return compile_error_ts(
                        "func!: comptime_label requires a non-empty string literal, \
                         e.g. comptime_label = \"fco_1_0\"",
                    );
                }
                args.pop();
                comptime_label = Some(label);
            }
        }
    }

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

    let (transformed, output_count, last_kind) = match transform_body(&body_text) {
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

    let mut compute = String::new();
    compute.push_str("let mut __fcomptime_val: Option<_> = None;\n    {\n");
    for (param, arg) in bindable.iter().zip(args.iter()) {
        compute.push_str(&format!("        let {} = {};\n", param, arg));
    }
    for line in transformed.lines() {
        compute.push_str("        ");
        compute.push_str(line);
        compute.push('\n');
    }
    compute.push_str("    }\n");
    compute.push_str("    let __fcomptime_v = match __fcomptime_val {\n");
    compute.push_str("        Some(__fcomptime_v) => __fcomptime_v,\n");
    compute.push_str(&format!("        None => panic!({:?}),\n", panic_msg));
    compute.push_str("    };\n");

    let mut code = format!("{{ {} ", rerun);
    match &comptime_label {
        Some(label) => {
            let is_str = last_kind == "str";
            code.push_str(&format!(
                "    #[cfg(test)]\n    {{\n{}        fcomptime::write_comptime_value(\
                 &__fcomptime_v, concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/comptime/\"), {:?}, {});\n\
                 \x20       __fcomptime_v\n    }}\n",
                compute, label, is_str
            ));
            code.push_str(&format!(
                "    #[cfg(not(test))]\n    fcomptime::comptime_include_expr!({:?})\n",
                label
            ));
        }
        None => {
            code.push_str(&compute);
            code.push_str("    __fcomptime_v\n");
        }
    }
    code.push('}');

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
        let (out, count, _kind) = transform_body(body).unwrap();
        assert_eq!(count, 1);
        assert!(out.contains("let res = i * 2;"));
        assert!(out.contains("__fcomptime_val = Some(res);"));
        assert!(!out.contains("output!"));
        assert!(!out.contains("source!"));
    }

    #[test]
    fn transform_body_handles_branches_and_multiple_outputs() {
        let body = "{\n    if i > 5 {\n        output!(raw, i, \"a\");\n    } else {\n        output!(raw, i * 3, \"b\");\n    }\n}";
        let (out, count, _kind) = transform_body(body).unwrap();
        assert_eq!(count, 2);
        assert!(out.contains("Some(i);"));
        assert!(out.contains("Some(i * 3);"));
    }

    #[test]
    fn transform_body_keeps_other_macros() {
        let body = "{\n    let x = 1;\n    call_scope! {\n        let y = 2;\n    }\n    println!(\"x\");\n}";
        let (out, count, _kind) = transform_body(body).unwrap();
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
        let (inner, count, _kind) = transform_body(body).unwrap();
        assert_eq!(count, 0);
        assert!(inner.contains("let a = 1;"));
    }

    #[test]
    fn transform_body_reports_last_output_kind() {
        let body = "{\n    source! {\n        let a = 1;\n        output!(str, format!(\"{}\", a), \"s\");\n    }\n}";
        let (_, _, kind) = transform_body(body).unwrap();
        assert_eq!(kind, "str");

        let body2 = "{\n    source! {\n        output!(raw, 5, \"r\");\n    }\n}";
        let (_, _, kind2) = transform_body(body2).unwrap();
        assert_eq!(kind2, "raw");
    }

    #[test]
    fn label_func_calls_in_text_assigns_labels_in_document_order() {
        let text = "call_scope! {\n    let a = func!(\"math\", 10);\n    let b = func!(\"scale\", a);\n    println!(\"{}\", b);\n}";
        let out = label_func_calls_in_text(text, 27).unwrap();
        assert!(out.contains("func!(\"math\", 10, comptime_label = \"fco_27_0\")"), "got: {}", out);
        assert!(out.contains("func!(\"scale\", a, comptime_label = \"fco_27_1\")"), "got: {}", out);
        assert!(out.contains("println!(\"{}\", b)"), "got: {}", out);
    }

    #[test]
    fn label_func_calls_in_text_handles_nested_calls() {
        let text = "let x = func!(\"outer\", func!(\"inner\", 5));";
        let out = label_func_calls_in_text(text, 9).unwrap();
        assert!(
            out.contains("func!(\"outer\", func!(\"inner\", 5), comptime_label = \"fco_9_0\");"),
            "got: {}",
            out
        );
        assert!(
            out.contains("func!(\"inner\", 5)") && !out.contains("fco_9_1"),
            "got: {}",
            out
        );
    }

    #[test]
    fn label_func_calls_in_text_strips_scope_wrapper() {
        let text = "call_scope! {\n    let a = func!(\"math\", 10);\n    let b = func!(\"scale\", a);\n}";
        let out = label_func_calls_in_text(text, 27).unwrap();
        assert!(out.contains("call_scope! {"), "got: {}", out);
        assert!(
            out.contains("func!(\"math\", 10, comptime_label = \"fco_27_0\")"),
            "got: {}",
            out
        );
        assert!(
            out.contains("func!(\"scale\", a, comptime_label = \"fco_27_1\")"),
            "got: {}",
            out
        );
    }

    #[test]
    fn build_fco_test_content_keeps_funcs_and_dependencies_only() {
        let body = "let base = 3;\nlet doubled = func!(\"math\", base);\nassert_eq!(doubled, 6);\nprintln!(\"{}\", doubled);";
        let (out, refs) = build_fco_test_content(body, 40).unwrap();
        assert!(out.contains("let base = 3;"), "got: {}", out);
        assert!(
            out.contains("func!(\"math\", base, comptime_label = \"fco_40_0\")"),
            "got: {}",
            out
        );
        assert!(!out.contains("assert_eq!(doubled, 6)"), "got: {}", out);
        assert!(!out.contains("println!(\"{}\", doubled)"), "got: {}", out);
        assert!(refs.contains("base"));
        assert!(!refs.contains("doubled"));
    }

    #[test]
    fn build_fco_test_content_rewrites_call_dependencies() {
        let body = "let seed = call!(\"data\");\nlet result = func!(\"scale\", seed);\nprintln!(\"seed: {}\", seed);";
        let (out, _) = build_fco_test_content(body, 12).unwrap();
        assert!(
            out.contains("let seed = fcomptime::runtime_read!(\"data\");"),
            "got: {}",
            out
        );
        assert!(
            out.contains("func!(\"scale\", seed, comptime_label = \"fco_12_0\")"),
            "got: {}",
            out
        );
        assert!(!out.contains("println!(\"seed: {}\", seed)"), "got: {}", out);
    }

    #[test]
    fn build_fco_test_content_strips_unused_call_reads() {
        let body = "let unused = call!(\"missing_file\");\nlet v = func!(\"math\", 1);\nprintln!(\"{}\", unused);";
        let (out, _) = build_fco_test_content(body, 3).unwrap();
        assert!(!out.contains("missing_file"), "got: {}", out);
        assert!(out.contains("func!(\"math\", 1, comptime_label = \"fco_3_0\")"), "got: {}", out);
    }

    #[test]
    fn filter_prefix_for_fco_keeps_only_dependencies() {
        let prefix = "let base = 100;\nlet noise = call!(\"nope\");\nlet x = 5;";
        let seed: HashSet<String> = ["base", "x"].iter().map(|s| s.to_string()).collect();
        let out = filter_prefix_for_fco(prefix, &seed);
        assert!(out.contains("let base = 100;"), "got: {}", out);
        assert!(out.contains("let x = 5;"), "got: {}", out);
        assert!(!out.contains("noise"), "got: {}", out);
        assert!(!out.contains("nope"), "got: {}", out);
    }

    #[test]
    fn call_rewrite_info_extracts_plain_and_token_names() {
        let text = "call!(\"data\")";
        let m = macro_invocations_in(text);
        assert_eq!(m.len(), 1);
        let (is, ie, name, ts, te) = &m[0];
        assert_eq!(name, "call");
        let info = call_rewrite_info(*is, *ie, *ts, *te, text).unwrap();
        assert_eq!(info.2, "data");

        let text2 = "call!(token, \"data\")";
        let m2 = macro_invocations_in(text2);
        let (is2, ie2, name2, ts2, te2) = &m2[0];
        assert_eq!(name2, "call");
        let info2 = call_rewrite_info(*is2, *ie2, *ts2, *te2, text2).unwrap();
        assert_eq!(info2.2, "data");

        let text3 = "call!(raw in, \"data\", let v { });";
        let m3 = macro_invocations_in(text3);
        let (is3, ie3, _, ts3, te3) = &m3[0];
        assert!(call_rewrite_info(*is3, *ie3, *ts3, *te3, text3).is_none());
    }

    #[test]
    fn is_inside_framework_macro_detects_nesting() {
        let text = "fn __t() {\n    call_scope! {\n        let v = func!(\"math\", 1);\n    }\n}";
        let mut p = Parser::new();
        p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
        let tree = p.parse(text, None).unwrap();
        let root = tree.root_node();
        let m = macro_invocations_in(text);
        assert_eq!(m.len(), 1, "func! inside the token tree is opaque to tree-sitter");
        let name_node = root
            .descendant_for_byte_range(m[0].0, m[0].1)
            .unwrap()
            .child_by_field_name("macro")
            .unwrap();
        assert!(!is_inside_framework_macro(name_node, text.as_bytes()));
    }
}
