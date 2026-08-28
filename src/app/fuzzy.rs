//! Fuzzy ranking for workspace-search candidates.
//!
//! Matching is delegated to `nucleo-matcher`, the low-level matcher used by
//! Helix and the `nucleo` picker. Keeping this adapter separate makes the
//! search state machine independent from a particular fuzzy algorithm.

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

pub(super) fn score(query: &str, candidate: &str) -> Option<u32> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buffer = Vec::new();
    Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    )
    .score(Utf32Str::new(candidate, &mut buffer), &mut matcher)
}

pub(super) fn atom_score(query: &str, candidate: &str) -> Option<u32> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buffer = Vec::new();
    nucleo_matcher::pattern::Atom::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    )
    .score(Utf32Str::new(candidate, &mut buffer), &mut matcher)
    .map(u32::from)
}

#[cfg(test)]
mod tests {
    use super::{atom_score, score};

    #[test]
    fn delegates_unicode_subsequence_matching_to_nucleo() {
        assert!(atom_score("mfn", "main_function").is_some());
        assert!(atom_score("方", "方法").is_some());
        assert!(atom_score("xyz", "main_function").is_none());
    }

    #[test]
    fn matches_all_space_separated_terms_against_candidate_metadata() {
        assert!(score("run service", "run Service /tmp/service.rs").is_some());
        assert!(score("run service", "run Controller /tmp/controller.rs").is_none());
    }
}
