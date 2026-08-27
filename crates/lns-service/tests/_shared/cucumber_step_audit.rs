use std::fs;
use std::path::{Path, PathBuf};

pub fn assert_no_dead_steps(crate_manifest_dir: &str) {
    let crate_dir = PathBuf::from(crate_manifest_dir);
    let steps_dir = crate_dir.join("tests").join("behaviours").join("steps");
    let features_root = crate_dir.join("tests").join("behaviours");

    let patterns = collect_step_patterns(&steps_dir);
    let phrases = collect_feature_phrases(&features_root);

    assert!(
        !patterns.is_empty(),
        "no step attributes found under {} — is the parser broken?",
        steps_dir.display()
    );
    assert!(
        !phrases.is_empty(),
        "no feature step phrases found under {} — is the parser broken?",
        features_root.display()
    );

    let mut dead: Vec<String> = Vec::new();
    for p in &patterns {
        let re = regex::Regex::new(&p.regex).unwrap_or_else(|e| {
            panic!(
                "step pattern at {}:{} is not a valid regex ({e}): {:?}",
                p.file.display(),
                p.line,
                p.regex,
            )
        });
        if !phrases.iter().any(|ph| re.is_match(ph)) {
            dead.push(format!(
                "  {}:{}  pattern: {:?}",
                p.file.display(),
                p.line,
                p.regex,
            ));
        }
    }

    assert!(
        dead.is_empty(),
        "dead step definitions (match no .feature phrase):\n{}",
        dead.join("\n")
    );
}

#[derive(Debug)]
struct StepPattern {
    regex: String,
    file: PathBuf,
    line: usize,
}

fn collect_step_patterns(dir: &Path) -> Vec<StepPattern> {
    let mut out = Vec::new();
    for entry in walk(dir) {
        if entry.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let src =
            fs::read_to_string(&entry).unwrap_or_else(|e| panic!("read {}: {e}", entry.display()));
        out.extend(parse_step_patterns(&entry, &src));
    }
    out
}

fn parse_step_patterns(file: &Path, src: &str) -> Vec<StepPattern> {
    const HEADS: &[&str] = &["#[given(", "#[when(", "#[then("];
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let Some((head_off, head)) = HEADS
            .iter()
            .filter_map(|h| src[i..].find(h).map(|off| (i + off, *h)))
            .min_by_key(|(off, _)| *off)
        else {
            break;
        };
        let body_start = head_off + head.len();
        let after_ws = skip_whitespace(src, body_start);
        // A body this parser does not know must never be dropped quietly: a skipped step shrinks the audit with no signal, which is how it came to pass steps it had never read.
        let Some(parsed) = parse_attr_body(src, after_ws) else {
            panic!(
                "{}:{}: step attribute body is not a spelling this audit understands: {:?}",
                file.display(),
                src[..head_off].lines().count() + 1,
                &src[after_ws..src.len().min(after_ws + 40)]
            )
        };
        let (pattern, end) = parsed;
        out.push(StepPattern {
            regex: pattern,
            file: file.to_path_buf(),
            line: src[..head_off].lines().count() + 1,
        });
        i = end;
    }
    out
}

fn parse_attr_body(src: &str, pos: usize) -> Option<(String, usize)> {
    let rest = src.get(pos..)?;
    for (head, hashes) in [("regex = r#\"", true), ("regex = r\"", false)] {
        if rest.starts_with(head) {
            let abs = pos + head.len();
            let close = if hashes { "\"#" } else { "\"" };
            let end_off = src[abs..].find(close)?;
            return Some((
                src[abs..abs + end_off].to_string(),
                abs + end_off + close.len(),
            ));
        }
    }
    if rest.starts_with("expr = \"") {
        let abs = pos + "expr = \"".len();
        let end_off = find_unescaped_quote(&src[abs..])?;
        let literal = unescape_rust_string(&src[abs..abs + end_off]);
        Some((expression_as_regex(&literal), abs + end_off + 1))
    } else if rest.starts_with("r#\"") {
        // A bare literal matches as written, whichever way it is quoted: cucumber does not read one as an expression, so the `/` in `text/markdown` is a slash and not an alternation.
        let abs = pos + "r#\"".len();
        let end_off = src[abs..].find("\"#")?;
        Some((
            format!("^{}$", regex::escape(&src[abs..abs + end_off])),
            abs + end_off + 2,
        ))
    } else if rest.starts_with('"') {
        let abs = pos + 1;
        let end_off = find_unescaped_quote(&src[abs..])?;
        let literal = unescape_rust_string(&src[abs..abs + end_off]);
        Some((format!("^{}$", regex::escape(&literal)), abs + end_off + 1))
    } else {
        None
    }
}

/// A cucumber expression as the regex cucumber itself would match it by, so this audit cannot drift from the runtime.
fn expression_as_regex(expr: &str) -> String {
    cucumber::codegen::Expression::regex(expr)
        .unwrap_or_else(|e| panic!("cucumber expression {expr:?} does not expand: {e}"))
        .as_str()
        .to_string()
}

fn skip_whitespace(src: &str, start: usize) -> usize {
    let bytes = src.as_bytes();
    let mut i = start;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    i
}

fn find_unescaped_quote(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn unescape_rust_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn collect_feature_phrases(dir: &Path) -> Vec<String> {
    const KEYWORDS: &[&str] = &["Given ", "When ", "Then ", "And ", "But ", "* "];
    let mut out = Vec::new();
    for entry in walk(dir) {
        if entry.extension().and_then(|s| s.to_str()) != Some("feature") {
            continue;
        }
        let src =
            fs::read_to_string(&entry).unwrap_or_else(|e| panic!("read {}: {e}", entry.display()));
        for line in src.lines() {
            let trimmed = line.trim_start();
            for kw in KEYWORDS {
                if let Some(rest) = trimmed.strip_prefix(kw) {
                    out.push(rest.trim_end().to_string());
                    break;
                }
            }
        }
    }
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let read = fs::read_dir(&d).unwrap_or_else(|e| panic!("read_dir {}: {e}", d.display()));
        for entry in read {
            let path = entry
                .unwrap_or_else(|e| panic!("dir entry under {}: {e}", d.display()))
                .path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns_of(src: &str) -> Vec<String> {
        parse_step_patterns(Path::new("fixture.rs"), src)
            .into_iter()
            .map(|p| p.regex)
            .collect()
    }

    #[test]
    fn a_bare_literal_matches_as_written_rather_than_as_an_expression() {
        // `text/markdown` is a slash in a real step phrase; read as an expression, cucumber's alternation would turn it into "text" or "markdown" and the step would read as dead.
        let re =
            regex::Regex::new(&patterns_of(r#"#[then("the README as a text/markdown layer")]"#)[0])
                .expect("valid regex");
        assert!(re.is_match("the README as a text/markdown layer"));
    }

    #[test]
    fn every_attribute_spelling_in_the_tree_is_read() {
        // A spelling this parser skips is a step the audit silently stops checking, which is the defect this module exists to prevent.
        let src = r###"
            #[given(regex = r#"^hashed (\d+)$"#)]
            #[when(regex = r"^bare raw$")]
            #[then(expr = "an expression {int}")]
            #[given("a plain literal")]
            #[when(r#"a raw literal with a "quote""#)]
        "###;
        assert_eq!(patterns_of(src).len(), 5);
    }

    #[test]
    #[should_panic(expected = "not a spelling this audit understands")]
    fn a_body_this_parser_cannot_read_is_loud() {
        patterns_of(r#"#[then(some_future_form = "x")]"#);
    }

    #[test]
    fn an_expression_expands_the_way_cucumber_matches_it() {
        // Taken from volume_cli.feature: a {string} argument may itself contain escaped quotes, which a naive [^"]* stops at.
        let re = regex::Regex::new(&expression_as_regex("the service refuses with {string}"))
            .expect("expands to a valid regex");
        assert!(
            re.is_match(r#"the service refuses with "volume \"prism-data\" in use by run #7""#)
        );
    }

    #[test]
    fn a_float_argument_accepts_what_cucumber_accepts() {
        // Delegating to cucumber's own expander is what keeps this true; a hand-rolled `-?\d+(?:\.\d+)?` rejects every one of these.
        let re = regex::Regex::new(&expression_as_regex("the delay is {float} seconds"))
            .expect("expands to a valid regex");
        for accepted in ["1e3", "+1", ".5", "1.", "inf", "NaN", "-2.5"] {
            assert!(
                re.is_match(&format!("the delay is {accepted} seconds")),
                "cucumber accepts {accepted:?} as a float, so this audit must too"
            );
        }
    }

    #[test]
    #[should_panic(expected = "does not expand")]
    fn an_untranslatable_parameter_is_loud_rather_than_matching_nothing() {
        expression_as_regex("a {nonexistent} parameter");
    }
}
