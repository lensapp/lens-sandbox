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
    document: Option<String>,
}

impl<D: Draw> Generator<D> {
    pub fn new(draw: D, document: Option<&str>) -> Self {
        Self {
            draw,
            document: document.map(str::to_string),
        }
    }
}

impl<D: Draw> Generate for Generator<D> {
    fn draw(&mut self) -> String {
        let noun = NOUNS[self.draw.index(NOUNS.len())];
        match &self.document {
            Some(document) => format!("{document}-{noun}"),
            None => {
                let adjective = ADJECTIVES[self.draw.index(ADJECTIVES.len())];
                format!("{adjective}-{noun}")
            }
        }
    }

    fn pool_size(&self) -> usize {
        match self.document {
            Some(_) => NOUNS.len(),
            None => ADJECTIVES.len() * NOUNS.len(),
        }
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
        Generator::new(ScriptedDraw(picks.to_vec()), None)
    }

    fn generator_for(document: &str, picks: &[usize]) -> Generator<ScriptedDraw> {
        Generator::new(ScriptedDraw(picks.to_vec()), Some(document))
    }

    #[test]
    fn a_generated_name_is_an_adjective_and_a_noun_joined_by_a_hyphen() {
        assert_eq!(generator(&[0, 0]).draw(), "amber-otter");
        assert_eq!(generator(&[1, 1]).draw(), "bold-falcon");
    }

    #[test]
    fn the_noun_is_drawn_before_the_adjective() {
        assert_eq!(generator(&[0, 1]).draw(), "bold-otter");
        assert_eq!(generator(&[1, 0]).draw(), "amber-falcon");
    }

    #[test]
    fn a_document_takes_the_place_of_the_adjective() {
        assert_eq!(
            generator_for("some-sandbox", &[0]).draw(),
            "some-sandbox-otter"
        );
        assert_eq!(
            generator_for("some-sandbox", &[1]).draw(),
            "some-sandbox-falcon"
        );
    }

    #[test]
    fn a_document_name_leaves_the_pool_one_word_wide() {
        assert_eq!(generator_for("some-sandbox", &[]).pool_size(), 50);
    }

    #[test]
    fn every_name_a_document_generates_is_a_legal_run_name() {
        for noun in 0..NOUNS.len() {
            let name = generator_for("some-sandbox", &[noun]).draw();
            lns_ipc::validate_run_name(&name)
                .unwrap_or_else(|e| panic!("generated name {name:?} is illegal: {e}"));
        }
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
        for noun in 0..NOUNS.len() {
            for adjective in 0..ADJECTIVES.len() {
                let name = generator(&[noun, adjective]).draw();
                lns_ipc::validate_run_name(&name)
                    .unwrap_or_else(|e| panic!("generated name {name:?} is illegal: {e}"));
            }
        }
    }

    #[test]
    fn the_pool_holds_pool_size_distinct_names() {
        let mut names = std::collections::HashSet::new();
        for noun in 0..NOUNS.len() {
            for adjective in 0..ADJECTIVES.len() {
                names.insert(generator(&[noun, adjective]).draw());
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
