//! Local fuzzy matcher for lists that carry no backend score — the Actions
//! section of Search Everywhere. Ported from this repo's own TypeScript
//! (`src/ui/match.ts` + `src/features/search/common/rank.ts`, Apache-2.0):
//! a contiguous substring beats word / camel-hump starts, which beat a plain
//! subsequence; the score orders exact > prefix > contiguous > word-starts >
//! spread. Indices are **char** indices (the TS original counted UTF-16
//! units) — the palette highlights by char runs.

/// One match with its ranking score and the matched char indices.
pub struct LocalMatch {
    pub score: i32,
    pub indices: Vec<usize>,
}

/// Multi-word aware match (the Find-Action rules): every whitespace-separated
/// query token must hit, and a scattered subsequence only counts when every
/// fragment after the first starts at a word / camel-hump boundary.
pub fn match_text(text: &str, query: &str) -> Option<LocalMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Some(LocalMatch { score: 0, indices: Vec::new() });
    }
    let text: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.iter().flat_map(|c| c.to_lowercase()).collect();
    let hit = |pattern: &str, text: &[char], lower: &[char]| -> Option<Vec<usize>> {
        let q: Vec<char> = pattern.chars().flat_map(|c| c.to_lowercase()).collect();
        fuzzy_match(text, lower, &q).filter(|indices| word_aligned(text, indices))
    };
    if let Some(indices) = hit(query, &text, &lower) {
        return Some(LocalMatch { score: score_indices(&text, query, &indices), indices });
    }
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }
    // Every token must match; the matched indices merge into one set.
    let mut all: Vec<usize> = Vec::new();
    let mut score = 0;
    for token in &tokens {
        let indices = hit(token, &text, &lower)?;
        all.extend(indices.iter().copied());
        score += score_indices(&text, token, &indices);
    }
    all.sort_unstable();
    all.dedup();
    Some(LocalMatch { score: score / tokens.len() as i32 - 50, indices: all })
}

/// Substring, then word / camel-hump starts, then a plain subsequence.
fn fuzzy_match(text: &[char], lower: &[char], query: &[char]) -> Option<Vec<usize>> {
    if query.is_empty() {
        return Some(Vec::new());
    }
    if let Some(at) = windows(lower, query.len()).position(|window| window == query) {
        return Some((at..at + query.len()).collect());
    }
    hump_match(text, lower, query).or_else(|| subsequence(lower, query))
}

fn windows(text: &[char], len: usize) -> impl Iterator<Item = &[char]> {
    (0..text.len().saturating_sub(len - 1)).map(move |at| &text[at..at + len])
}

fn is_word_start(text: &[char], at: usize) -> bool {
    match at {
        0 => true,
        _ => {
            let (prev, c) = (text[at - 1], text[at]);
            matches!(prev, ' ' | '\t' | '_' | '-' | '.' | '/' | '\\' | ':')
                || (c.is_uppercase() && !prev.is_uppercase())
        }
    }
}

/// Camel-hump / word-start greedy match ("OCo" matches "OrderController").
fn hump_match(text: &[char], lower: &[char], query: &[char]) -> Option<Vec<usize>> {
    let mut out = Vec::new();
    let (mut qi, mut i) = (0, 0);
    while qi < query.len() && i < text.len() {
        let starts = is_word_start(text, i);
        let contiguous = i > 0 && out.last() == Some(&(i - 1));
        if lower[i] == query[qi] && (if qi == 0 { starts } else { contiguous || starts }) {
            out.push(i);
            qi += 1;
        }
        i += 1;
    }
    (qi == query.len()).then_some(out)
}

fn subsequence(lower: &[char], query: &[char]) -> Option<Vec<usize>> {
    let mut out = Vec::new();
    let mut qi = 0;
    for (i, c) in lower.iter().enumerate() {
        if qi < query.len() && c == &query[qi] {
            out.push(i);
            qi += 1;
        }
    }
    (qi == query.len()).then_some(out)
}

/// Contiguous hits count; otherwise every fragment after the first must start
/// at a boundary (IDEA's Find-Action rule: "main" must not match "sMArt step
/// INto").
fn word_aligned(text: &[char], indices: &[usize]) -> bool {
    if contiguous(indices) {
        return true;
    }
    for (k, &i) in indices.iter().enumerate() {
        if k > 0 && indices[k - 1] == i - 1 {
            continue;
        }
        if i != 0 && !is_word_start(text, i) {
            return false;
        }
    }
    true
}

/// Consecutive indices (each one past its predecessor).
fn contiguous(indices: &[usize]) -> bool {
    indices.windows(2).all(|window| window[1] == window[0] + 1)
}

fn score_indices(text: &[char], query: &str, indices: &[usize]) -> i32 {
    let lower: String = text.iter().collect::<String>().to_lowercase();
    let q = query.to_lowercase();
    let score = if lower == q {
        1000
    } else if lower.starts_with(&q) {
        800
    } else if contiguous(indices) {
        let first = indices.first().copied().unwrap_or(0) as i32;
        600 - first.min(100) * 2
    } else {
        let starts = indices.iter().filter(|&&i| i == 0 || is_word_start(text, i)).count() as i32;
        if starts >= 2.min(indices.len() as i32) { 400 + starts * 10 } else { 200 - spread(indices) }
    };
    let case_exact = i32::from(
        text.get(indices.first().copied().unwrap_or(0)..)
            .is_some_and(|rest| rest.iter().collect::<String>().starts_with(query)),
    ) * 5;
    score + case_exact - 60.min(text.len() as i32)
}

fn spread(indices: &[usize]) -> i32 {
    let (Some(first), Some(last)) = (indices.first(), indices.last()) else { return 0 };
    // Sorted distinct indices span at least their count; saturating keeps a
    // broken invariant from wrapping in release.
    ((last - first + 1).saturating_sub(indices.len()) as i32).min(150)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_of(text: &str, query: &str) -> i32 {
        match_text(text, query).expect("should match").score
    }

    #[test]
    fn empty_query_matches_everything() {
        let hit = match_text("Anything", "  ").expect("empty query matches");
        assert_eq!(hit.score, 0);
        assert!(hit.indices.is_empty());
    }

    #[test]
    fn ranking_prefers_exact_then_prefix_then_contiguous_then_hump() {
        let exact = score_of("open", "open");
        let prefix = score_of("open file", "open");
        // A lowercase-prefix hit on a CamelHump name still counts as prefix
        // (the TS ranker lowercases both sides); a truly scattered hump hit
        // ranks below a mid-text contiguous run.
        let contiguous = score_of("recently open", "open");
        let hump = score_of("OpenProjectFile", "opf");
        assert!(exact > prefix && prefix > contiguous && contiguous > hump, "{exact} {prefix} {contiguous} {hump}");
    }

    #[test]
    fn camel_hump_matches_word_starts() {
        let hit = match_text("OrderController", "OCont").expect("hump match");
        assert_eq!(hit.indices, vec![0, 5, 6, 7, 8]);
    }

    #[test]
    fn scattered_subsequence_needs_word_alignment() {
        // Fragments landing mid-word are rejected ("man" inside "domain"),
        // like IDEA's Find Action.
        assert!(match_text("domain step", "man").is_none());
        // Word-start fragments pass ("Ma" + "Co" at the camel humps).
        assert!(match_text("MainConfig", "MaCo").is_some());
    }

    #[test]
    fn multi_word_query_needs_every_token() {
        assert!(match_text("open recent file", "open file").is_some());
        assert!(match_text("open recent", "open file").is_none());
    }

    #[test]
    fn case_exact_beats_same_shape() {
        // The case-exact bonus only applies when the text matches the query's
        // own casing at the match point.
        assert!(score_of("Open File", "Open") > score_of("open file", "Open"));
    }

    #[test]
    fn indices_highlight_the_matched_chars() {
        let hit = match_text("Goto Definition", "goto").expect("prefix match");
        let highlighted: String = hit.indices.iter().map(|&i| "Goto Definition".chars().nth(i).unwrap()).collect();
        assert_eq!(highlighted, "Goto");
    }
}
