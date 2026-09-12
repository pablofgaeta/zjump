//! A subsequence matcher, sized for picking among tens of targets.
//!
//! Rolled by hand rather than pulled from `nucleo`/`fuzzy-matcher` to keep the
//! spike dependency-free and to find out whether the scoring is good enough at
//! this scale. Swapping it for a real matcher later is a one-function change.

/// Word starts score higher, so "zs" hits "zellij/switcher" over "puzzles".
const BOUNDARIES: [char; 5] = ['/', '-', '_', '.', ' '];

pub struct Match {
    pub score: i32,
    /// Char indices into the haystack, for highlighting.
    pub indices: Vec<usize>,
}

/// Scores `needle` as a subsequence of `haystack`, case-insensitively.
///
/// Returns `None` when the needle is not a subsequence at all. An empty needle
/// matches everything with a score of 0, which is what makes the unfiltered
/// list fall back to its natural order.
pub fn match_query(haystack: &str, needle: &str) -> Option<Match> {
    if needle.is_empty() {
        return Some(Match {
            score: 0,
            indices: Vec::new(),
        });
    }

    let hay: Vec<char> = haystack.chars().collect();
    let mut indices = Vec::new();
    let mut score = 0;
    let mut hay_idx = 0;
    let mut last_hit: Option<usize> = None;

    for want in needle.chars().flat_map(|c| c.to_lowercase()) {
        let hit = loop {
            let candidate = *hay.get(hay_idx)?;
            hay_idx += 1;
            if candidate.to_lowercase().eq(std::iter::once(want)) {
                break hay_idx - 1;
            }
        };

        score += 1;
        // Outweighs the consecutive-run bonus deliberately: "as" should rank
        // "agent/status" over "aspects".
        if hit == 0 || BOUNDARIES.contains(&hay[hit - 1]) {
            score += 10;
        }
        match last_hit {
            Some(previous) if hit == previous + 1 => score += 5,
            // Distance penalty, floored so a far-apart match still beats no match.
            Some(previous) => score -= ((hit - previous) as i32 - 1).min(3),
            None => {}
        }
        last_hit = Some(hit);
        indices.push(hit);
    }

    // Shorter haystacks win ties: an exact-ish name beats a long one containing it.
    score -= (hay.len() as i32) / 16;
    Some(Match { score, indices })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(haystack: &str, needle: &str) -> Option<i32> {
        match_query(haystack, needle).map(|m| m.score)
    }

    #[test]
    fn rejects_a_non_subsequence() {
        assert!(score("zellij", "zzz").is_none());
        assert!(score("", "a").is_none());
    }

    #[test]
    fn matches_a_subsequence_out_of_order_chars_aside() {
        assert!(score("zellij/switcher", "zs").is_some());
        assert!(score("zellij/switcher", "sz").is_none());
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(score("Zellij", "zel"), score("zellij", "ZEL"));
    }

    #[test]
    fn empty_needle_matches_everything_neutrally() {
        let matched = match_query("anything", "").unwrap();
        assert_eq!(matched.score, 0);
        assert!(matched.indices.is_empty());
    }

    #[test]
    fn prefers_word_boundaries_over_mid_word_hits() {
        let boundary = score("agent/status", "as").unwrap();
        let mid_word = score("aspects", "as").unwrap();
        assert!(
            boundary > mid_word,
            "boundary {} should beat mid-word {}",
            boundary,
            mid_word
        );
    }

    #[test]
    fn prefers_consecutive_runs() {
        let consecutive = score("switcher", "swi").unwrap();
        let scattered = score("saxwaxix", "swi").unwrap();
        assert!(
            consecutive > scattered,
            "consecutive {} should beat scattered {}",
            consecutive,
            scattered
        );
    }

    #[test]
    fn reports_indices_for_highlighting() {
        let matched = match_query("zellij", "zlj").unwrap();
        assert_eq!(matched.indices, vec![0, 2, 5]);
    }
}
