use std::cmp::Reverse;

use super::{SearchItem, SearchState};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
// The derived lexicographic ordering encodes the UX contract: exact, prefix,
// and substring matches beat subsequences; compact and early matches then win.
struct FuzzyScore {
    match_quality: u8,
    consecutive_pairs: usize,
    gap_penalty: Reverse<usize>,
    start_penalty: Reverse<usize>,
    length_penalty: Reverse<usize>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SearchScore {
    whole_query_matches_symbol: bool,
    symbol: FuzzyScore,
    container: Option<FuzzyScore>,
}

pub(super) fn refresh_search_items(search: &mut SearchState) {
    let mut matches = search
        .candidates
        .iter()
        .enumerate()
        .filter_map(|(index, item)| search_score(&search.input, item).map(|score| (index, score)))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| {
                search.candidates[left.0]
                    .name
                    .to_lowercase()
                    .cmp(&search.candidates[right.0].name.to_lowercase())
            })
            .then_with(|| left.0.cmp(&right.0))
    });

    search.items = matches
        .into_iter()
        .map(|(index, _)| search.candidates[index].clone())
        .collect();
    search.selected = (!search.items.is_empty()).then_some(0);
}

fn search_score(query: &str, item: &SearchItem) -> Option<SearchScore> {
    if let Some(symbol) = fuzzy_score(query, &item.name) {
        return Some(SearchScore {
            whole_query_matches_symbol: true,
            symbol,
            container: None,
        });
    }

    let mut parts = query.split_whitespace();
    let symbol_query = parts.next()?;
    let container_query = parts.collect::<Vec<_>>().join(" ");
    if container_query.is_empty() {
        return None;
    }

    let symbol = fuzzy_score(symbol_query, &item.name)?;
    let container_label = item.container_name.as_deref().map_or_else(
        || item.location.clone(),
        |container| format!("{container} {}", item.location),
    );
    let container = fuzzy_score(&container_query, &container_label)?;
    Some(SearchScore {
        whole_query_matches_symbol: false,
        symbol,
        container: Some(container),
    })
}

fn fuzzy_score(query: &str, candidate: &str) -> Option<FuzzyScore> {
    let query = query.to_lowercase();
    let candidate = candidate.to_lowercase();
    let query_chars = query.chars().collect::<Vec<_>>();
    let candidate_chars = candidate.chars().collect::<Vec<_>>();

    if query_chars.is_empty() {
        return Some(FuzzyScore {
            match_quality: 0,
            consecutive_pairs: 0,
            gap_penalty: Reverse(0),
            start_penalty: Reverse(0),
            length_penalty: Reverse(candidate_chars.len()),
        });
    }

    let mut matched_positions = Vec::with_capacity(query_chars.len());
    let mut query_index = 0;
    for (candidate_index, character) in candidate_chars.iter().enumerate() {
        if query_chars.get(query_index) == Some(character) {
            matched_positions.push(candidate_index);
            query_index += 1;
            if query_index == query_chars.len() {
                break;
            }
        }
    }
    if query_index != query_chars.len() {
        return None;
    }

    let first = matched_positions[0];
    let last = *matched_positions
        .last()
        .expect("a non-empty query has a match");
    let span = last - first + 1;
    let consecutive_pairs = matched_positions
        .windows(2)
        .filter(|positions| positions[1] == positions[0] + 1)
        .count();
    let match_quality = if query == candidate {
        3
    } else if candidate.starts_with(&query) {
        2
    } else if candidate.contains(&query) {
        1
    } else {
        0
    };

    Some(FuzzyScore {
        match_quality,
        consecutive_pairs,
        gap_penalty: Reverse(span - query_chars.len()),
        start_penalty: Reverse(first),
        length_penalty: Reverse(candidate_chars.len()),
    })
}
