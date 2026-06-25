const ADJECTIVES: &[&str] = &[
    "amber", "bold", "calm", "dapper", "eager", "fuzzy", "gentle", "hardy", "ivory", "jolly",
    "keen", "lucid", "mellow", "nimble", "olive", "plucky", "quiet", "rapid", "sleek", "tidy",
    "upbeat", "vivid", "witty", "zesty",
];

const NOUNS: &[&str] = &[
    "otter", "falcon", "maple", "comet", "harbor", "lynx", "quartz", "willow", "ember", "badger",
    "cedar", "marlin", "puffin", "ridge", "sparrow", "thistle", "walrus", "yak", "zephyr",
    "beacon", "cypress", "delta", "grove", "heron",
];

pub fn name_for_index(i: usize) -> String {
    let adjective = ADJECTIVES[i % ADJECTIVES.len()];
    let noun = NOUNS[(i / ADJECTIVES.len()) % NOUNS.len()];
    format!("{adjective}_{noun}")
}

pub fn pool_size() -> usize {
    ADJECTIVES.len() * NOUNS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_for_index_is_adjective_underscore_noun() {
        let name = name_for_index(0);
        let (adj, noun) = name.split_once('_').expect("adjective_noun shape");
        assert!(ADJECTIVES.contains(&adj));
        assert!(NOUNS.contains(&noun));
    }

    #[test]
    fn consecutive_indices_vary_the_adjective_before_repeating_a_noun() {
        assert_ne!(name_for_index(0), name_for_index(1));
        assert_eq!(
            name_for_index(0),
            name_for_index(ADJECTIVES.len() * NOUNS.len())
        );
    }

    #[test]
    fn an_auto_name_is_never_all_digits() {
        for i in [0, 1, 7, 23, 24, 600] {
            assert!(lns_ipc::validate_run_name(&name_for_index(i)).is_ok());
        }
    }

    #[test]
    fn the_pool_holds_pool_size_distinct_names_before_repeating() {
        let names: std::collections::HashSet<String> =
            (0..pool_size()).map(name_for_index).collect();
        assert_eq!(names.len(), pool_size());
        assert_eq!(name_for_index(0), name_for_index(pool_size()));
    }
}
