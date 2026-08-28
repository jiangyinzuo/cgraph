use super::fuzzy;
use super::{SearchItem, SearchState};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SearchScore {
    symbol: u32,
    trailing: Option<u32>,
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
    let mut parts = query.split_whitespace();
    let Some(symbol_query) = parts.next() else {
        return Some(SearchScore {
            symbol: 0,
            trailing: None,
        });
    };
    let symbol = fuzzy::atom_score(symbol_query, &item.name)?;
    let trailing_query = parts.collect::<Vec<_>>().join(" ");
    let trailing = if trailing_query.is_empty() {
        None
    } else {
        let searchable = item.container_name.as_deref().map_or_else(
            || format!("{} {}", item.name, item.location),
            |container| format!("{} {container} {}", item.name, item.location),
        );
        Some(fuzzy::score(&trailing_query, &searchable)?)
    };
    Some(SearchScore { symbol, trailing })
}
