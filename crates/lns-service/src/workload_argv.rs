pub fn from_image_config(
    config: &oci_client::config::ConfigFile,
    override_cmd: &[String],
) -> String {
    let cfg = config.config.as_ref();
    let entrypoint = cfg.and_then(|c| c.entrypoint.clone()).unwrap_or_default();
    let image_cmd = cfg.and_then(|c| c.cmd.clone()).unwrap_or_default();

    let argv: Vec<String> = if !override_cmd.is_empty() {
        override_cmd.to_vec()
    } else {
        entrypoint.into_iter().chain(image_cmd).collect()
    };

    shell_quote_argv(&argv)
}

pub fn shell_quote_argv(argv: &[String]) -> String {
    if argv.is_empty() {
        return "sh".to_string();
    }
    argv.iter()
        .map(|s| shell_quote(s))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '=' | '+' | ',')
    }) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_safe_chars_pass_through() {
        assert_eq!(shell_quote("foo"), "foo");
        assert_eq!(shell_quote("-p"), "-p");
    }

    #[test]
    fn shell_quote_spaces_and_specials() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("a&b"), "'a&b'");
    }

    #[test]
    fn shell_quote_empty_becomes_pair_of_single_quotes() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_embedded_single_quote_uses_close_escape_reopen() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_argv_empty_input_returns_sh() {
        assert_eq!(shell_quote_argv(&[]), "sh");
    }

    #[test]
    fn shell_quote_argv_joins_quoted_tokens_with_spaces() {
        let argv = vec!["/bin/echo".to_string(), "hi there".to_string()];
        assert_eq!(shell_quote_argv(&argv), "/bin/echo 'hi there'");
    }

    #[test]
    fn from_image_config_uses_override_when_present() {
        let config = oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: Some(oci_client::config::Config {
                entrypoint: Some(vec!["/entry".into()]),
                cmd: Some(vec!["default".into()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), "echo hi".into()];
        assert_eq!(
            from_image_config(&config, &argv),
            "/bin/sh -c 'echo hi'",
            "non-empty override must shadow entrypoint+cmd entirely",
        );
    }

    #[test]
    fn from_image_config_concatenates_entrypoint_and_cmd_when_no_override() {
        let config = oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: Some(oci_client::config::Config {
                entrypoint: Some(vec!["/entry".into(), "--flag".into()]),
                cmd: Some(vec!["arg1".into(), "arg with space".into()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            from_image_config(&config, &[]),
            "/entry --flag arg1 'arg with space'",
            "empty override should yield entrypoint ++ cmd",
        );
    }

    #[test]
    fn from_image_config_handles_missing_config_section() {
        let config = oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: None,
            ..Default::default()
        };
        assert_eq!(from_image_config(&config, &[]), "sh");
    }
}
