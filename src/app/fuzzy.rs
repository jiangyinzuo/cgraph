//! Fuzzy ranking for workspace-search candidates.
//!
//! Matching is delegated to `nucleo-matcher`, the low-level matcher used by
//! Helix and the `nucleo` picker. Keeping this adapter separate makes the
//! search state machine independent from a particular fuzzy algorithm.

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization},
};

/// Scores one fuzzy subsequence after discarding query whitespace.
///
/// Spaces are only visual separators: `parser thread` is matched as
/// `parserthread`, not parsed into independent boolean terms.
pub(super) fn score(query: &str, candidate: &str) -> Option<u32> {
    let compact_query = query
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    atom_score(&compact_query, candidate)
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
    fn treats_spaces_as_readable_fuzzy_query_separators() {
        assert!(score("prs thrd", "ParserService::thread_worker").is_some());
        assert!(score("thrd prs", "ParserService::thread_worker").is_none());
    }
}
