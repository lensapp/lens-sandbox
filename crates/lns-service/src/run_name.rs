const ADJECTIVES: &[&str] = &[
    "amber", "bold", "calm", "dapper", "eager", "fuzzy", "gentle", "hardy", "ivory", "jolly",
    "keen", "lucid", "mellow", "nimble", "olive", "plucky", "quiet", "rapid", "sleek", "tidy",
    "upbeat", "vivid", "witty", "zesty",
];

const NOUNS: &[&str] = &[
    "otter", "falcon", "maple", "comet", "harbor", "lynx", "quartz", "willow", "ember", "badger",
    "cedar", "marlin", "puffin", "ridge", "sparrow", "thistle", "walrus", "yak", "zephyr",
    "beacon", "cypress", "delta", "grove", "heron", "alder", "anchor", "aspen", "bison", "canyon",
    "coral", "dune", "fjord", "geyser", "glacier", "hollow", "indigo", "juniper", "kestrel",
    "lagoon", "meadow", "nimbus", "orchid", "pelican", "prairie", "reef", "summit", "tundra",
    "vireo", "wombat", "yarrow",
];

pub trait Draw {
    fn index(&mut self, len: usize) -> usize;
}

pub struct ThreadDraw;

impl Draw for ThreadDraw {
    fn index(&mut self, len: usize) -> usize {
        rand::Rng::gen_range(&mut rand::thread_rng(), 0..len)
    }
}

pub trait Generate {
    fn draw(&mut self) -> String;
    fn pool_size(&self) -> usize;
}

pub struct Generator<D> {
    draw: D,
}

impl<D: Draw> Generator<D> {
    pub fn new(draw: D) -> Self {
        Self { draw }
    }
}

impl<D: Draw> Generate for Generator<D> {
    fn draw(&mut self) -> String {
        let adjective = ADJECTIVES[self.draw.index(ADJECTIVES.len())];
        let noun = NOUNS[self.draw.index(NOUNS.len())];
        format!("{adjective}-{noun}")
    }

    fn pool_size(&self) -> usize {
        ADJECTIVES.len() * NOUNS.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScriptedDraw(Vec<usize>);

    impl Draw for ScriptedDraw {
        fn index(&mut self, len: usize) -> usize {
            self.0.remove(0) % len
        }
    }

    fn generator(picks: &[usize]) -> Generator<ScriptedDraw> {
        Generator::new(ScriptedDraw(picks.to_vec()))
    }

    #[test]
    fn a_generated_name_is_an_adjective_and_a_noun_joined_by_a_hyphen() {
        assert_eq!(generator(&[0, 0]).draw(), "amber-otter");
        assert_eq!(generator(&[1, 1]).draw(), "bold-falcon");
    }

    #[test]
    fn the_adjective_is_drawn_before_the_noun() {
        assert_eq!(generator(&[0, 1]).draw(), "amber-falcon");
        assert_eq!(generator(&[1, 0]).draw(), "bold-otter");
    }

    #[test]
    fn the_noun_pool_holds_fifty_words() {
        assert_eq!(NOUNS.len(), 50);
        let distinct: std::collections::HashSet<_> = NOUNS.iter().collect();
        assert_eq!(distinct.len(), NOUNS.len());
    }

    #[test]
    fn the_generated_pool_holds_twelve_hundred_names() {
        assert_eq!(generator(&[]).pool_size(), 1200);
    }

    #[test]
    fn every_generated_name_is_a_legal_run_name() {
        for adjective in 0..ADJECTIVES.len() {
            for noun in 0..NOUNS.len() {
                let name = generator(&[adjective, noun]).draw();
                lns_ipc::validate_run_name(&name)
                    .unwrap_or_else(|e| panic!("generated name {name:?} is illegal: {e}"));
            }
        }
    }

    #[test]
    fn the_pool_holds_pool_size_distinct_names() {
        let mut names = std::collections::HashSet::new();
        for adjective in 0..ADJECTIVES.len() {
            for noun in 0..NOUNS.len() {
                names.insert(generator(&[adjective, noun]).draw());
            }
        }
        assert_eq!(names.len(), generator(&[]).pool_size());
    }

    #[test]
    fn thread_draw_stays_inside_the_pool() {
        let mut draw = ThreadDraw;
        for _ in 0..200 {
            assert!(draw.index(NOUNS.len()) < NOUNS.len());
        }
    }

    #[test]
    fn thread_draw_does_not_hand_out_the_same_word_every_time() {
        let mut draw = ThreadDraw;
        let seen: std::collections::HashSet<usize> =
            (0..200).map(|_| draw.index(NOUNS.len())).collect();
        assert!(seen.len() > 1, "every draw returned the same index");
    }
}
