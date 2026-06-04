use anyhow::Result;
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        let prog = args
            .first()
            .map(String::as_str)
            .unwrap_or("coverage-strip-ast");
        eprintln!("usage: {prog} <lcov.info>");
        std::process::exit(2);
    }
    coverage_strip_ast::run(Path::new(&args[1]))
}
