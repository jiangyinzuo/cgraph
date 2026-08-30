use super::fuzzy;
use super::{SearchItem, SearchState};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SearchScore {
    symbol: Option<u32>,
    uri: Option<u32>,
}

pub(super) fn refresh_search_items(search: &mut SearchState) {
    let mut matches = search
        .candidates
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            search_score(&search.symbol_query, &search.uri_query, item).map(|score| (index, score))
        })
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

fn search_score(symbol_query: &str, uri_query: &str, item: &SearchItem) -> Option<SearchScore> {
    Some(SearchScore {
        symbol: optional_score(symbol_query, &item.name)?,
        uri: optional_uri_score(uri_query, item)?,
    })
}

fn optional_score(query: &str, candidate: &str) -> Option<Option<u32>> {
    if query.split_whitespace().next().is_none() {
        Some(None)
    } else {
        fuzzy::score(query, candidate).map(Some)
    }
}

fn optional_uri_score(query: &str, item: &SearchItem) -> Option<Option<u32>> {
    if query.split_whitespace().next().is_none() {
        return Some(None);
    }
    let location_score = fuzzy::score(query, &item.location);
    let source_score = item
        .source
        .as_ref()
        .and_then(|source| fuzzy::score(query, &source.uri));
    location_score.max(source_score).map(Some)
}
