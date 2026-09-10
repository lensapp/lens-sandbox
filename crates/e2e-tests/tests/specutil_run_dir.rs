#[path = "specutil/mod.rs"]
mod specutil;

use specutil::run_dir_name_for_prefix;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| (*n).to_string()).collect()
}

#[test]
fn the_printed_short_id_resolves_to_the_run_directory_it_names() {
    let dirs = names(&[
        "e93a758c2062de82a4b091204f9d5cbe",
        "1f0c44aa0b1e4d5f8c6a2b3d4e5f6071",
    ]);

    let resolved = run_dir_name_for_prefix(&dirs, "e93a758c2062");

    assert_eq!(
        resolved.as_deref(),
        Ok("e93a758c2062de82a4b091204f9d5cbe"),
        "the full id the service wrote under is the one the prefix names"
    );
}

#[test]
fn a_prefix_no_run_directory_carries_is_refused_by_name() {
    let dirs = names(&["1f0c44aa0b1e4d5f8c6a2b3d4e5f6071"]);

    let error = run_dir_name_for_prefix(&dirs, "e93a758c2062").expect_err("no candidate matches");

    assert!(
        error.contains("no run directory starts with") && error.contains("e93a758c2062"),
        "the failure must name the prefix it looked for, got {error:?}"
    );
    assert!(
        error.contains("1f0c44aa0b1e4d5f8c6a2b3d4e5f6071"),
        "the failure must show what was there instead, got {error:?}"
    );
}

#[test]
fn two_run_directories_under_one_prefix_are_refused_instead_of_guessed() {
    let dirs = names(&[
        "e93a758c2062de82a4b091204f9d5cbe",
        "e93a758c2062ff11a4b091204f9d5cbe",
    ]);

    let error = run_dir_name_for_prefix(&dirs, "e93a758c2062").expect_err("two candidates match");

    assert!(
        error.starts_with("2 run directories start with"),
        "an ambiguous prefix must say how many matched, got {error:?}"
    );
}
