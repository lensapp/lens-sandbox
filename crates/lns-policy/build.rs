use std::collections::BTreeSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const MANIFEST: &str = "src/connectors.yaml";

fn main() {
    println!("cargo:rerun-if-changed={MANIFEST}");
    let yaml = fs::read_to_string(MANIFEST).expect("connectors.yaml must be readable");

    let mut generated = String::from("pub(crate) static ENV_SUBSTITUTIONS: &[(&str, &str)] = &[\n");
    for name in scan_env_refs(&yaml) {
        println!("cargo:rerun-if-env-changed={name}");
        let value = env::var(&name).unwrap_or_default();
        writeln!(generated, "    ({name:?}, {value:?}),").unwrap();
    }
    generated.push_str("];\n");

    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("env_substitutions.rs");
    fs::write(out, generated).expect("writing env_substitutions.rs");
}

fn scan_env_refs(src: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut rest = src;
    while let Some(open) = rest.find("${") {
        rest = &rest[open + 2..];
        let Some(close) = rest.find('}') else { break };
        let name = &rest[..close];
        if is_env_ident(name) {
            names.insert(name.to_string());
        }
        rest = &rest[close + 1..];
    }
    names
}

fn is_env_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
