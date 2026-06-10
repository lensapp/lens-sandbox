/// Replaces each literal `${NAME}` in `src` with its mapped value; an unmapped reference is left untouched.
pub(crate) fn apply_substitutions(src: &str, subs: &[(&str, &str)]) -> String {
    let mut out = src.to_string();
    for (name, value) in subs {
        out = out.replace(&format!("${{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_without_references_is_returned_unchanged() {
        assert_eq!(apply_substitutions("k: plain", &[]), "k: plain");
    }

    #[test]
    fn a_present_reference_is_replaced_with_its_value() {
        assert_eq!(
            apply_substitutions("k: \"${SOME_VAR}\"", &[("SOME_VAR", "SOME_TOKEN")]),
            "k: \"SOME_TOKEN\""
        );
    }

    #[test]
    fn a_reference_mapped_to_empty_becomes_an_empty_string() {
        assert_eq!(
            apply_substitutions("k: \"${SOME_VAR}\"", &[("SOME_VAR", "")]),
            "k: \"\""
        );
    }

    #[test]
    fn a_repeated_reference_is_replaced_everywhere() {
        assert_eq!(
            apply_substitutions("${SOME_VAR}-${SOME_VAR}", &[("SOME_VAR", "x")]),
            "x-x"
        );
    }

    #[test]
    fn multiple_distinct_references_each_resolve() {
        assert_eq!(
            apply_substitutions("a=${ONE} b=${TWO}", &[("ONE", "1"), ("TWO", "2")]),
            "a=1 b=2"
        );
    }

    #[test]
    fn an_unmapped_reference_is_left_literal() {
        assert_eq!(apply_substitutions("k: ${SOME_VAR}", &[]), "k: ${SOME_VAR}");
    }

    #[test]
    fn a_substitution_whose_name_is_absent_from_the_source_is_a_no_op() {
        assert_eq!(apply_substitutions("plain", &[("ABSENT", "v")]), "plain");
    }
}
