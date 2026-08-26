/// The grammar a variable the guest environment has to be able to hold obeys.
pub fn is_legal_env_var_name(name: &str) -> bool {
    !name.is_empty()
        && !name
            .chars()
            .any(|c| c == '=' || c.is_control() || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legal_env_var_name_is_non_empty_and_holds_no_separator_or_control() {
        assert!(is_legal_env_var_name("SOME_TOKEN"));
        assert!(!is_legal_env_var_name(""));
        assert!(!is_legal_env_var_name("SOME=TOKEN"));
        assert!(!is_legal_env_var_name("SOME TOKEN"));
        assert!(!is_legal_env_var_name("SOME\u{7f}TOKEN"));
    }
}
