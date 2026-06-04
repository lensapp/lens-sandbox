/// Append shell-quoted CLI args to the agent command so the joined string runs verbatim under `sh -c`.
pub(super) fn append_cli_args(agent_command: &mut String, args: &[String]) {
    for arg in args {
        agent_command.push(' ');
        agent_command.push_str(&shell_quote(arg));
    }
}

/// Shell-quote a string for safe inclusion in `sh -c` commands.
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
    fn shell_quote_simple() {
        assert_eq!(shell_quote("-p"), "-p");
        assert_eq!(shell_quote("hello"), "hello");
    }

    #[test]
    fn shell_quote_with_spaces() {
        assert_eq!(shell_quote("fix the tests"), "'fix the tests'");
    }

    #[test]
    fn shell_quote_with_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_empty() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_special_chars() {
        assert_eq!(shell_quote("a&b"), "'a&b'");
        assert_eq!(shell_quote("$(cmd)"), "'$(cmd)'");
    }

    #[test]
    fn append_cli_args_quotes_and_joins() {
        let mut cmd = "claude".to_string();
        append_cli_args(&mut cmd, &["-p".into(), "fix the tests".into()]);
        assert_eq!(cmd, "claude -p 'fix the tests'");
    }

    #[test]
    fn append_cli_args_empty_is_noop() {
        let mut cmd = "sh".to_string();
        append_cli_args(&mut cmd, &[]);
        assert_eq!(cmd, "sh");
    }
}
