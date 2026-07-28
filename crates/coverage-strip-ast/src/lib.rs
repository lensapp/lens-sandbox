use anyhow::{Context, Result};
use proc_macro2::LineColumn;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::Visit;

pub fn run(lcov_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(lcov_path)
        .with_context(|| format!("reading {}", lcov_path.display()))?;
    let output = strip(&content);
    std::fs::write(lcov_path, output)
        .with_context(|| format!("writing {}", lcov_path.display()))?;
    Ok(())
}

fn strip(input: &str) -> String {
    let mut output = String::new();
    let mut section: Option<Section> = None;

    for raw in input.lines() {
        if let Some(sf) = raw.strip_prefix("SF:") {
            if let Some(s) = section.take() {
                s.flush(&mut output);
            }
            section = Some(Section::new(PathBuf::from(sf)));
            output.push_str(raw);
            output.push('\n');
        } else if let Some(rest) = raw.strip_prefix("DA:") {
            let parts: Vec<&str> = rest.splitn(3, ',').collect();
            if parts.len() < 2 {
                output.push_str(raw);
                output.push('\n');
                continue;
            }
            let line_num: usize = parts[0].parse().unwrap_or(0);
            let count: u64 = parts[1].parse().unwrap_or(0);
            if let Some(sec) = section.as_mut() {
                sec.record_da(line_num, count, &mut output);
            } else {
                output.push_str(raw);
                output.push('\n');
            }
        } else if raw.starts_with("LF:") || raw.starts_with("LH:") {
        } else if raw == "end_of_record" {
            if let Some(s) = section.take() {
                s.flush_lf_lh(&mut output);
            }
            output.push_str("end_of_record\n");
        } else {
            output.push_str(raw);
            output.push('\n');
        }
    }
    if let Some(s) = section.take() {
        s.flush(&mut output);
    }
    output
}

struct Section {
    file: PathBuf,
    classification: Option<LineClassification>,
    da_unique: BTreeSet<usize>,
    da_hit: BTreeSet<usize>,
}

impl Section {
    fn new(file: PathBuf) -> Self {
        Self {
            file,
            classification: None,
            da_unique: BTreeSet::new(),
            da_hit: BTreeSet::new(),
        }
    }

    fn record_da(&mut self, line: usize, count: u64, output: &mut String) {
        let classification = self
            .classification
            .get_or_insert_with(|| LineClassification::for_source(&self.file));
        if classification.is_executable(line) {
            self.da_unique.insert(line);
            if count > 0 {
                self.da_hit.insert(line);
            }
            output.push_str(&format!("DA:{line},{count}\n"));
        }
    }

    fn flush_lf_lh(&self, output: &mut String) {
        output.push_str(&format!("LF:{}\n", self.da_unique.len()));
        output.push_str(&format!("LH:{}\n", self.da_hit.len()));
    }

    fn flush(self, output: &mut String) {
        self.flush_lf_lh(output);
    }
}

struct LineClassification {
    marker_excluded: BTreeSet<usize>,
    ast_executable: BTreeSet<usize>,
    source_lines: usize,
    fallback_keep_all: bool,
}

impl LineClassification {
    fn for_source(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::fallback();
        };
        let source_lines = content.lines().count();
        let marker_excluded = collect_marker_excluded_lines(&content);
        let ast = match syn::parse_file(&content) {
            Ok(a) => a,
            Err(_) => {
                return Self {
                    marker_excluded,
                    ast_executable: BTreeSet::new(),
                    source_lines,
                    fallback_keep_all: true,
                };
            }
        };
        let mut collector = ExecutableLineCollector::new(&content);
        collector.visit_file(&ast);
        Self {
            marker_excluded,
            ast_executable: collector.lines,
            source_lines,
            fallback_keep_all: false,
        }
    }

    fn fallback() -> Self {
        Self {
            marker_excluded: BTreeSet::new(),
            ast_executable: BTreeSet::new(),
            source_lines: 0,
            fallback_keep_all: false,
        }
    }

    fn is_executable(&self, line: usize) -> bool {
        if self.marker_excluded.contains(&line) {
            return false;
        }
        if self.fallback_keep_all {
            return true;
        }
        // LLVM emits phantom DA entries past EOF from stale monomorphized source maps.
        if line == 0 || line > self.source_lines {
            return false;
        }
        self.ast_executable.contains(&line)
    }
}

fn collect_marker_excluded_lines(source: &str) -> BTreeSet<usize> {
    let mut excl = BTreeSet::new();
    let mut in_block = false;
    for (idx, line) in source.lines().enumerate() {
        let lineno = idx + 1;
        if line_has_marker(line, &["coverage:ignore-end", "LCOV_EXCL_STOP"]) {
            in_block = false;
            continue;
        }
        if line_has_marker(line, &["coverage:ignore-start", "LCOV_EXCL_START"]) {
            in_block = true;
            continue;
        }
        if in_block {
            excl.insert(lineno);
            continue;
        }
        if line_has_marker(line, &["coverage:ignore-line", "LCOV_EXCL_LINE"]) {
            excl.insert(lineno);
        }
    }
    excl
}

fn line_has_marker(line: &str, keywords: &[&str]) -> bool {
    for kw in keywords {
        if let Some(idx) = line.find(kw) {
            let prefix = &line[..idx];
            if !prefix.contains("//") {
                continue;
            }
            let after = &line[idx + kw.len()..];
            let next_byte = after.bytes().next();
            let ok = match next_byte {
                None => true,
                Some(b) => !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            };
            if ok {
                return true;
            }
        }
    }
    false
}

struct ExecutableLineCollector<'a> {
    lines: BTreeSet<usize>,
    phantom_entry_lines: BTreeSet<usize>,
    inert_macro_arg_lines: BTreeSet<usize>,
    macro_expression_lines: BTreeSet<usize>,
    source_lines: Vec<&'a str>,
}

impl<'a> ExecutableLineCollector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            lines: BTreeSet::new(),
            phantom_entry_lines: BTreeSet::new(),
            inert_macro_arg_lines: BTreeSet::new(),
            macro_expression_lines: BTreeSet::new(),
            source_lines: source.lines().collect(),
        }
    }

    /// A function's opening-brace line carries an LLVM entry counter that intermittently reports 0 even when the body ran; the body lines already catch a never-called fn, so drop it when the body starts on a later line.
    fn note_fn_body(&mut self, block: &syn::Block) {
        let open_line = block.brace_token.span.open().start().line;
        if let Some(first) = block.stmts.first()
            && first.span().start().line > open_line
        {
            self.phantom_entry_lines.insert(open_line);
        }
    }

    fn note_sequential_if_guard_phantom(&mut self, block: &syn::Block) {
        for pair in block.stmts.windows(2) {
            let syn::Stmt::Expr(syn::Expr::If(head), _) = &pair[0] else {
                continue;
            };
            if !matches!(&pair[1], syn::Stmt::Expr(syn::Expr::If(_), _)) {
                continue;
            }
            if head.else_branch.is_some() || !block_diverges(&head.then_branch) {
                continue;
            }
            let open_line = head.then_branch.brace_token.span.open().start().line;
            if head
                .then_branch
                .stmts
                .first()
                .is_some_and(|s| s.span().start().line > open_line)
            {
                self.phantom_entry_lines.insert(open_line);
            }
        }
    }

    fn mark_line(&mut self, line: usize) {
        if line == 0 {
            return;
        }
        let Some(text) = self.source_lines.get(line - 1) else {
            return;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if trimmed.starts_with("//") {
            return;
        }
        // An attribute on its own line (e.g. `#[cfg(unix)]` above a statement) is compile-time, not executable, yet LLVM emits a phantom 0-count region there.
        if trimmed.starts_with("#[") || trimmed.starts_with("#![") {
            return;
        }
        if is_scaffolding_only(trimmed) {
            return;
        }
        self.lines.insert(line);
    }

    fn mark_span(&mut self, start: LineColumn, end: LineColumn) {
        for line in start.line..=end.line {
            self.mark_line(line);
        }
    }

    /// Notes which of a macro invocation's argument lines carry nothing evaluable — only literals and punctuation, as a format string on its own line does. An enclosing expression's span marks every line it covers, so these are collected here and dropped once the walk is done.
    fn note_inert_macro_arg_lines(&mut self, m: &syn::Macro) {
        let mut expression_lines = BTreeSet::new();
        collect_expression_token_lines(m.tokens.clone(), &mut expression_lines);
        let Some(arg_lines) = token_span_lines(&m.tokens) else {
            return;
        };
        let path_line = m.path.span().start().line;
        for line in arg_lines {
            if line != path_line && !expression_lines.contains(&line) {
                self.inert_macro_arg_lines.insert(line);
            }
        }
        self.macro_expression_lines.extend(expression_lines);
    }
}

/// The lines a macro's argument tokens span, first token to last; `None` for an invocation with no arguments.
fn token_span_lines(tokens: &proc_macro2::TokenStream) -> Option<std::ops::RangeInclusive<usize>> {
    let mut iter = tokens.clone().into_iter();
    let first = iter.next()?;
    let last = iter.last().unwrap_or_else(|| first.clone());
    Some(first.span().start().line..=last.span().end().line)
}

/// The lines of a macro's arguments bearing a token that can evaluate to something — anything but a literal or punctuation, descending into groups so a nested call is not missed.
fn collect_expression_token_lines(tokens: proc_macro2::TokenStream, out: &mut BTreeSet<usize>) {
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Group(group) => {
                collect_expression_token_lines(group.stream(), out);
            }
            proc_macro2::TokenTree::Literal(_) | proc_macro2::TokenTree::Punct(_) => {}
            other => {
                let span = other.span();
                for line in span.start().line..=span.end().line {
                    out.insert(line);
                }
            }
        }
    }
}

/// True when a line carries only block and call scaffolding — closers, openers, separators, or the `else` keyword — so no expression sits on it to execute. An `else` branch that is never taken still leaves its body uncovered, so dropping the `} else {` line costs no signal.
fn is_scaffolding_only(trimmed: &str) -> bool {
    let residue: String = trimmed
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '{' | '}' | '(' | ')' | '[' | ']' | ';' | ',' | '?' | ' ' | '\t'
            )
        })
        .collect();
    residue.is_empty() || residue == "else"
}

impl<'ast, 'a> Visit<'ast> for ExecutableLineCollector<'a> {
    fn visit_file(&mut self, file: &'ast syn::File) {
        syn::visit::visit_file(self, file);
        for line in std::mem::take(&mut self.phantom_entry_lines) {
            self.lines.remove(&line);
        }
        // Another invocation on the same line may have put an expression there, so only a line inert everywhere goes.
        for line in std::mem::take(&mut self.inert_macro_arg_lines) {
            if !self.macro_expression_lines.contains(&line) {
                self.lines.remove(&line);
            }
        }
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        let span = block.brace_token.span;
        self.mark_span(span.open().start(), span.close().end());
        self.note_sequential_if_guard_phantom(block);
        syn::visit::visit_block(self, block);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.mark_span(item.sig.fn_token.span.start(), item.sig.span().end());
        self.note_fn_body(&item.block);
        syn::visit::visit_item_fn(self, item);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        self.mark_span(item.sig.fn_token.span.start(), item.sig.span().end());
        self.note_fn_body(&item.block);
        syn::visit::visit_impl_item_fn(self, item);
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        self.mark_span(item.sig.fn_token.span.start(), item.sig.span().end());
        if let Some(block) = &item.default {
            self.note_fn_body(block);
        }
        syn::visit::visit_trait_item_fn(self, item);
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        let span = stmt.span();
        self.mark_line(span.start().line);
        syn::visit::visit_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        let span = expr.span();
        self.mark_span(span.start(), span.end());
        syn::visit::visit_expr(self, expr);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        // macro_rules bodies are templates — never executed at the definition site.
        if is_macro_rules_path(&m.path) {
            return;
        }
        let tail_end = m
            .tokens
            .clone()
            .into_iter()
            .last()
            .map(|t| t.span().end())
            .unwrap_or_else(|| m.path.span().end());
        self.mark_span(m.path.span().start(), tail_end);
        self.note_inert_macro_arg_lines(m);
        syn::visit::visit_macro(self, m);
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if is_macro_rules_path(&item.mac.path) {
            return;
        }
        syn::visit::visit_item_macro(self, item);
    }

    fn visit_attribute(&mut self, _attr: &'ast syn::Attribute) {}
}

fn is_macro_rules_path(p: &syn::Path) -> bool {
    p.leading_colon.is_none()
        && p.segments.len() == 1
        && p.segments
            .first()
            .map(|s| s.ident == "macro_rules")
            .unwrap_or(false)
}

fn block_diverges(block: &syn::Block) -> bool {
    match block.stmts.last() {
        Some(syn::Stmt::Expr(
            syn::Expr::Return(_) | syn::Expr::Break(_) | syn::Expr::Continue(_),
            _,
        )) => true,
        Some(syn::Stmt::Macro(m)) => matches!(
            m.mac
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .as_deref(),
            Some("panic" | "unreachable" | "todo" | "unimplemented")
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn run_strips_in_place_and_recomputes_counts() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("sample.rs");
        std::fs::write(&source, "fn f() {\n    let x = 1;\n    let _ = x;\n}\n").unwrap();
        let lcov = dir.path().join("cov.info");
        std::fs::write(
            &lcov,
            format!(
                "SF:{}\nDA:1,1\nDA:2,1\nDA:4,0\nLF:99\nLH:99\nend_of_record\n",
                source.display()
            ),
        )
        .unwrap();

        run(&lcov).unwrap();

        let out = std::fs::read_to_string(&lcov).unwrap();
        assert!(
            !out.contains("DA:1,1"),
            "fn signature dropped (entry counter)"
        );
        assert!(out.contains("DA:2,1"), "body stmt kept");
        assert!(!out.contains("DA:4,0"), "closing brace dropped");
        assert!(out.contains("LF:1"), "LF recomputed from survivors");
        assert!(out.contains("LH:1"));
    }

    #[test]
    fn collector_drops_only_diverging_sequential_if_guard_phantoms() {
        let src = r#"fn d(n: u32) -> u32 {
    if n > 10 {
        return 0;
    }
    if n == 9 {
        break;
    }
    if n == 8 {
        continue;
    }
    if n == 7 {
        panic!("x");
    }
    if n == 1 {
        side();
    }
    if n == 5 {
        do_it();
    } else {
        nope();
    }
    if n == 0 {
        return 1;
    }
    let y = n;
    if y > 5 {
        return 2;
    }
    y
}
"#;
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(!c.lines.contains(&2), "return-guard header dropped");
        assert!(!c.lines.contains(&5), "break-guard header dropped");
        assert!(!c.lines.contains(&8), "continue-guard header dropped");
        assert!(!c.lines.contains(&11), "panic-guard header dropped");
        assert!(
            c.lines.contains(&14),
            "non-diverging if header kept so a real uncovered branch can't be hidden"
        );
        assert!(c.lines.contains(&17), "if-with-else header kept");
        assert!(c.lines.contains(&22), "diverging guard before a let kept");
        assert!(
            c.lines.contains(&26),
            "diverging guard before trailing expr kept"
        );
        assert!(c.lines.contains(&3), "guard body kept");
    }

    #[test]
    fn run_surfaces_read_failure_with_path_context() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.info");
        let err = run(&missing).unwrap_err();
        assert!(format!("{err:#}").contains("reading"));
    }

    #[test]
    fn run_surfaces_write_failure_with_path_context() {
        let dir = tempfile::tempdir().unwrap();
        let lcov = dir.path().join("cov.info");
        std::fs::write(&lcov, "TN:\nend_of_record\n").unwrap();
        let mut perms = std::fs::metadata(&lcov).unwrap().permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&lcov, perms).unwrap();

        let err = run(&lcov).unwrap_err();
        assert!(format!("{err:#}").contains("writing"));
    }

    #[test]
    fn strip_flushes_section_when_next_sf_begins() {
        let input = "SF:/no/a.rs\nDA:1,1\nSF:/no/b.rs\nDA:1,1\nend_of_record\n";
        let out = strip(input);
        assert_eq!(out.matches("LF:").count(), 2, "both sections flushed");
    }

    #[test]
    fn strip_flushes_trailing_section_without_end_of_record() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a.rs");
        std::fs::write(&source, "fn f() {\n    let x = 1;\n}\n").unwrap();
        let input = format!("SF:{}\nDA:2,1\n", source.display());
        let out = strip(&input);
        assert!(out.contains("LF:1"), "trailing open section flushed");
        assert!(out.contains("LH:1"));
    }

    #[test]
    fn strip_passes_malformed_da_through_untouched() {
        let input = "SF:/no/a.rs\nDA:5\nend_of_record\n";
        let out = strip(input);
        assert!(out.contains("DA:5\n"), "DA missing count passed through");
    }

    #[test]
    fn strip_passes_da_without_section_through() {
        let input = "DA:1,1\nend_of_record\n";
        let out = strip(input);
        assert!(out.contains("DA:1,1"), "DA before any SF passed through");
    }

    #[test]
    fn strip_keeps_all_da_for_unparseable_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("broken.rs");
        std::fs::write(&source, "this is @@@ not valid rust\n").unwrap();
        let input = format!("SF:{}\nDA:1,0\nend_of_record\n", source.display());
        let out = strip(&input);
        assert!(out.contains("DA:1,0"), "unparseable source keeps every DA");
    }

    #[test]
    fn strip_drops_all_da_for_missing_source_file() {
        let input = "TN:\nSF:/nonexistent/path.rs\nDA:1,5\nLF:1\nLH:1\nend_of_record\n";
        let out = strip(input);
        assert!(
            !out.contains("DA:1,5"),
            "missing source file must not keep DA entries (phantom path)"
        );
    }

    #[test]
    fn is_executable_marker_exclusion_wins_over_ast() {
        let cls = LineClassification {
            marker_excluded: BTreeSet::from([2]),
            ast_executable: BTreeSet::from([2]),
            source_lines: 3,
            fallback_keep_all: false,
        };
        assert!(!cls.is_executable(2), "marker exclusion drops an AST line");
    }

    #[test]
    fn classification_drops_da_beyond_source_bounds() {
        let cls = LineClassification {
            marker_excluded: BTreeSet::new(),
            ast_executable: BTreeSet::from([1, 2, 3]),
            source_lines: 3,
            fallback_keep_all: false,
        };
        assert!(cls.is_executable(2));
        assert!(!cls.is_executable(4), "line past EOF must be dropped");
        assert!(!cls.is_executable(0), "line 0 must be dropped");
    }

    #[test]
    fn marker_excl_recognises_ignore_line() {
        let src = "let x = 1; // coverage:ignore-line because reasons\nlet y = 2;\n";
        let excl = collect_marker_excluded_lines(src);
        assert!(excl.contains(&1));
        assert!(!excl.contains(&2));
    }

    #[test]
    fn marker_excl_recognises_block() {
        let src = "let a = 1;\n// coverage:ignore-start reason\nlet b = 2;\nlet c = 3;\n// coverage:ignore-end\nlet d = 4;\n";
        let excl = collect_marker_excluded_lines(src);
        assert!(!excl.contains(&1));
        assert!(excl.contains(&3));
        assert!(excl.contains(&4));
        assert!(!excl.contains(&6));
    }

    #[test]
    fn marker_excl_lcov_aliases_work() {
        let src =
            "// LCOV_EXCL_START\nlet x = 1;\n// LCOV_EXCL_STOP\nlet y = 2; // LCOV_EXCL_LINE\n";
        let excl = collect_marker_excluded_lines(src);
        assert!(excl.contains(&2));
        assert!(excl.contains(&4));
    }

    #[test]
    fn marker_ignored_when_keyword_is_not_in_a_comment() {
        let src = "let s = \"coverage:ignore-line\";\nlet y = 2;\n";
        let excl = collect_marker_excluded_lines(src);
        assert!(
            excl.is_empty(),
            "keyword outside a // comment is not a marker"
        );
    }

    #[test]
    fn line_marker_does_not_match_substring() {
        let src = "// coverage:ignore-lineage other\nlet x = 1;\n";
        let excl = collect_marker_excluded_lines(src);
        assert!(excl.is_empty());
    }

    #[test]
    fn collector_marks_function_body_lines() {
        let src = "/// docs\nfn f() {\n    let x = 1;\n    x + 1\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(!c.lines.contains(&1), "doc comment line");
        assert!(
            !c.lines.contains(&2),
            "fn signature excluded (entry counter)"
        );
        assert!(c.lines.contains(&3), "body stmt");
    }

    #[test]
    fn collector_marks_impl_method_body_lines() {
        let src = "struct S;\nimpl S {\n    fn m(&self) {\n        let x = 1;\n        let _ = x;\n    }\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(
            !c.lines.contains(&3),
            "method signature excluded (entry counter)"
        );
        assert!(c.lines.contains(&4), "method body stmt");
        assert!(c.lines.contains(&5), "method body stmt");
    }

    #[test]
    fn collector_skips_standalone_attribute_lines() {
        // Pin: an attribute on its own line (`#[cfg(unix)]` above a
        // statement) is not marked executable, even though the
        // statement's span starts on the attribute line. LLVM emits a
        // 0-count region there that would otherwise sink the file.
        let src = "fn f() {\n    #[cfg(unix)]\n    let _ = 1;\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(!c.lines.contains(&1), "fn signature line is excluded");
        assert!(!c.lines.contains(&2), "`#[cfg(unix)]` attribute line");
        assert!(c.lines.contains(&3), "guarded statement");
    }

    #[test]
    fn collector_excludes_multiline_fn_signature_but_keeps_single_line_body() {
        // A multi-line fn's `fn name() {` line is excluded (its LLVM entry counter can read 0 while the body ran); a single-line fn keeps its line because the body is on it.
        let src = "fn multi() {\n    do_work();\n}\nfn single() { done() }\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(!c.lines.contains(&1), "multi-line fn signature excluded");
        assert!(c.lines.contains(&2), "multi-line fn body statement");
        assert!(c.lines.contains(&4), "single-line fn keeps its body line");
    }

    #[test]
    fn collector_excludes_trait_default_method_signature_and_marks_its_body() {
        // A trait default method goes through visit_trait_item_fn: its multi-line signature line is excluded like any fn, and the default body's statements are marked.
        let src =
            "trait T {\n    fn d(&self) {\n        let x = 1;\n        let _ = x;\n    }\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(!c.lines.contains(&1), "trait header line");
        assert!(
            !c.lines.contains(&2),
            "default-method signature excluded (entry counter)"
        );
        assert!(c.lines.contains(&3), "default-method body stmt");
        assert!(c.lines.contains(&4), "default-method body stmt");
    }

    #[test]
    fn collector_skips_struct_only_lines() {
        let src = "/// docs for foo\nstruct Foo;\n\nimpl Trait for Foo {}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(!c.lines.contains(&1));
        assert!(!c.lines.contains(&2));
        assert!(!c.lines.contains(&3));
        assert!(!c.lines.contains(&4));
    }

    #[test]
    fn collector_skips_pure_closer_lines() {
        let src =
            "fn f(opt: Option<u32>) {\n    if let Some(x) = opt {\n        let _ = x;\n    }\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(!c.lines.contains(&1), "fn signature line is excluded");
        assert!(c.lines.contains(&2), "if let line");
        assert!(c.lines.contains(&3), "body stmt");
        assert!(!c.lines.contains(&4), "`    }}` is pure punctuation");
        assert!(!c.lines.contains(&5), "`}}` is pure punctuation");
    }

    #[test]
    fn collector_skips_multiline_call_close_punctuation() {
        let src = "fn f() -> Result<(), ()> {\n    write_at(\n        1,\n        2,\n    )?;\n    Ok(())\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(c.lines.contains(&2), "call-site line");
        assert!(c.lines.contains(&3), "arg line");
        assert!(c.lines.contains(&4), "arg line");
        assert!(!c.lines.contains(&5), "`    )?;` is pure punctuation");
    }

    #[test]
    fn collector_skips_a_macro_argument_line_holding_only_a_format_string() {
        // LLVM maps an expansion region onto the format-string line and reports it uncovered while the `bail!(` line above it counts the calls, so demanding coverage there demands the impossible.
        let src = "fn f(name: &str) -> Result<(), String> {\n    if name.is_empty() {\n        bail!(\n            \"credential {:?} must name its env var\",\n            name\n        );\n    }\n    Ok(())\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(
            c.lines.contains(&3),
            "the `bail!(` invocation line executes"
        );
        assert!(
            !c.lines.contains(&4),
            "a line carrying only a format string has no expression on it to execute"
        );
        assert!(
            c.lines.contains(&5),
            "an argument line carrying an expression is still demanded"
        );
    }

    #[test]
    fn collector_marks_expressions_nested_in_a_macro_argument_group() {
        let src = "fn f() {\n    println!(\n        \"{}\",\n        compute(\n            input()\n        )\n    );\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(!c.lines.contains(&3), "format-string-only line");
        assert!(c.lines.contains(&4), "call inside the macro arguments");
        assert!(
            c.lines.contains(&5),
            "a call nested in a parenthesised group is reached by descending into it"
        );
    }

    #[test]
    fn collector_marks_an_argument_less_macro_invocation() {
        let src = "fn f() {\n    unreachable!();\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(
            c.lines.contains(&2),
            "an invocation with no arguments has no argument line to judge, and the call itself still executes"
        );
    }

    #[test]
    fn collector_skips_an_else_scaffolding_line() {
        // LLVM sometimes maps the else branch's region onto `} else {`, which holds no expression; an else that is never taken still leaves its body uncovered, so the signal survives.
        let src =
            "fn f(c: bool) -> u32 {\n    if c {\n        1\n    } else {\n        2\n    }\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(c.lines.contains(&2), "the `if` line branches");
        assert!(c.lines.contains(&3), "the then value");
        assert!(!c.lines.contains(&4), "`}} else {{` is scaffolding");
        assert!(
            c.lines.contains(&5),
            "the else value still carries the branch's coverage"
        );
    }

    #[test]
    fn scaffolding_only_keeps_an_else_if_condition() {
        assert!(is_scaffolding_only("} else {"));
        assert!(is_scaffolding_only("    }"));
        assert!(is_scaffolding_only("{"));
        assert!(
            !is_scaffolding_only("} else if c.is_ready() {"),
            "an `else if` line carries a condition that is evaluated, so it stays measured"
        );
        assert!(!is_scaffolding_only("let x = 1;"));
    }

    #[test]
    fn collector_marks_normal_item_macro_invocation() {
        let src = "mymacro! {\n    a b c\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(c.lines.contains(&1), "macro call site marked");
    }

    #[test]
    fn collector_skips_macro_rules_definition() {
        let src = "macro_rules! gen {\n    () => {\n        let x = 1;\n    };\n}\n";
        let ast = syn::parse_file(src).unwrap();
        let mut c = ExecutableLineCollector::new(src);
        c.visit_file(&ast);
        assert!(
            c.lines.is_empty(),
            "macro_rules template lines never marked"
        );
    }

    #[test]
    fn collector_visit_macro_skips_macro_rules_path_directly() {
        let item: syn::ItemMacro = syn::parse_str("macro_rules! gen { () => {}; }").unwrap();
        let mut c = ExecutableLineCollector::new("macro_rules! gen { () => {}; }");
        c.visit_macro(&item.mac);
        assert!(c.lines.is_empty(), "a macro_rules-path Macro is skipped");
    }

    #[test]
    fn mark_line_rejects_non_executable_positions() {
        let src = "fn f() {}\n\n    // a comment\nlet x = 1;\n";
        let mut c = ExecutableLineCollector::new(src);
        c.mark_line(0);
        c.mark_line(9999);
        c.mark_line(2);
        c.mark_line(3);
        assert!(
            c.lines.is_empty(),
            "line 0, OOB, blank, comment all rejected"
        );
        c.mark_line(4);
        assert!(c.lines.contains(&4), "real statement line is marked");
    }

    #[test]
    fn is_macro_rules_path_distinguishes_paths() {
        let mac: syn::ItemMacro = syn::parse_str("macro_rules! gen { () => {}; }").unwrap();
        assert!(is_macro_rules_path(&mac.mac.path));
        let other: syn::ItemMacro = syn::parse_str("other! { x }").unwrap();
        assert!(!is_macro_rules_path(&other.mac.path));
    }
}
