pub fn mount_entry(path: &str, rw: bool) -> String {
    let mode = if rw { "rw" } else { "ro" };
    if path.contains(':') {
        if path.ends_with(":ro") || path.ends_with(":rw") {
            path.to_string()
        } else {
            format!("{path}:{mode}")
        }
    } else {
        format!("{path}:{path}:{mode}")
    }
}

pub fn looks_secret_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".aws",
        ".ssh",
        ".netrc",
        ".gnupg",
        "/secrets",
        "credentials",
        "id_rsa",
        ".npmrc",
        ".pypirc",
        ".docker/config",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub fn cc_box_names(ls_output: &str) -> Vec<String> {
    ls_output
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter(|name| name.starts_with("cc-"))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_entry_defaults_read_only_and_expands_bare_paths() {
        assert_eq!(mount_entry("/data", false), "/data:/data:ro");
        assert_eq!(mount_entry("/data", true), "/data:/data:rw");
        assert_eq!(mount_entry("/host:/guest", false), "/host:/guest:ro");
        assert_eq!(mount_entry("/host:/guest:ro", true), "/host:/guest:ro");
    }

    #[test]
    fn secret_paths_are_refused() {
        assert!(looks_secret_path("/Users/me/.aws/credentials"));
        assert!(looks_secret_path("~/.ssh/id_rsa"));
        assert!(looks_secret_path("/home/me/.netrc"));
        assert!(!looks_secret_path("/Users/me/data"));
        assert!(!looks_secret_path("/work/project"));
    }

    #[test]
    fn cc_box_names_extracts_plugin_boxes_only() {
        let ls = "\
ID  NAME          STATUS        IMAGE          COMMAND            STARTED
1   cc-python-ab  running       python:3.12    bash -lc x         now
2   calm_otter    exited (0)    alpine:3.20    sh                 now
3   cc-node-cd    exited (7)    node:22        bash -lc y         now";
        assert_eq!(
            cc_box_names(ls),
            vec!["cc-python-ab".to_string(), "cc-node-cd".to_string()]
        );
    }

    #[test]
    fn cc_box_names_handles_empty_listing() {
        assert!(cc_box_names("ID  NAME  STATUS").is_empty());
        assert!(cc_box_names("").is_empty());
    }
}
