//! Fuzzy subsequence scoring.
//!
//! The scoring model is the one from the upstream `tobi/try` spec:
//!
//! - Each query character must appear in order in the candidate text;
//!   missing any character disqualifies the candidate.
//! - Per match: `+1.0`, plus `+1.0` when the match is at a word boundary
//!   (start of string or after a non-alphanumeric character), plus a
//!   proximity bonus `2 / sqrt(gap + 1)` against the previous match.
//! - The accumulated fuzzy score is then multiplied by a density factor
//!   `query_len / (last_match_pos + 1)` and a length penalty
//!   `10 / (name_len + 10)`.
//! - The caller-supplied `base_score` (date + recency bonus) is added *after*
//!   the multipliers — it is purely additive, never scaled.
//!
//! For an empty query, every entry matches with `score = base_score` and no
//! highlight positions.

use std::cmp::Ordering;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub text: String,
    pub base_score: f64,
    text_lower: String,
    char_count: usize,
}

impl Candidate {
    pub fn new(text: impl Into<String>, base_score: f64) -> Self {
        let text = text.into();
        let text_lower = text.to_lowercase();
        let char_count = text.chars().count();
        Self {
            text,
            text_lower,
            char_count,
            base_score,
        }
    }
}

/// A scored fuzzy match.
///
/// `positions` are byte indices into the *lowercased* candidate text, which
/// for ASCII identifiers is identical to indices into the original string.
/// Callers that need to highlight non-ASCII names should map back through
/// `char_indices()`.
#[derive(Debug, Clone)]
pub struct Hit {
    pub index: usize,
    pub score: f64,
    pub positions: Vec<usize>,
}

#[derive(Debug)]
pub struct Matcher<'a> {
    candidates: &'a [Candidate],
}

impl<'a> Matcher<'a> {
    pub fn new(candidates: &'a [Candidate]) -> Self {
        Self { candidates }
    }

    /// Score every candidate against `query`, returning hits sorted by descending score.
    ///
    /// When `limit > 0`, only the top `limit` hits are returned.
    pub fn query(&self, query: &str, limit: usize) -> Vec<Hit> {
        let query_lower = query.to_lowercase();

        let mut hits: Vec<Hit> = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(index, c)| {
                score_one(&c.text_lower, c.char_count, c.base_score, &query_lower).map(
                    |(score, positions)| Hit {
                        index,
                        score,
                        positions,
                    },
                )
            })
            .collect();

        hits.sort_unstable_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        if limit > 0 && limit < hits.len() {
            hits.truncate(limit);
        }
        hits
    }
}

fn score_one(
    text_lower: &str,
    name_char_count: usize,
    base_score: f64,
    query_lower: &str,
) -> Option<(f64, Vec<usize>)> {
    if query_lower.is_empty() {
        return Some((base_score, Vec::new()));
    }

    let text_chars: Vec<char> = text_lower.chars().collect();
    let query_chars: Vec<char> = query_lower.chars().collect();
    let query_len = query_chars.len();

    let mut positions = Vec::with_capacity(query_len);
    let mut fuzzy = 0.0_f64;
    let mut last_pos: Option<usize> = None;
    let mut cursor = 0_usize;

    for &qc in &query_chars {
        let found = text_chars[cursor..].iter().position(|&c| c == qc)? + cursor;

        positions.push(found);
        fuzzy += 1.0;

        let is_boundary = found == 0 || !text_chars[found - 1].is_alphanumeric();
        if is_boundary {
            fuzzy += 1.0;
        }

        if let Some(prev) = last_pos {
            let gap = found - prev - 1;
            fuzzy += 2.0 / ((gap as f64) + 1.0).sqrt();
        }

        last_pos = Some(found);
        cursor = found + 1;
    }

    let last = last_pos.expect("non-empty query yields at least one match");
    fuzzy *= query_len as f64 / (last + 1) as f64;
    fuzzy *= 10.0 / (name_char_count as f64 + 10.0);

    Some((fuzzy + base_score, positions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn empty_query_returns_base_score_only() {
        let cands = vec![Candidate::new("foo", 1.5), Candidate::new("bar", 3.0)];
        let hits = Matcher::new(&cands).query("", 0);
        assert_eq!(hits.len(), 2);
        // Sorted descending by score.
        assert_eq!(hits[0].index, 1);
        approx(hits[0].score, 3.0);
        assert!(hits[0].positions.is_empty());
    }

    #[test]
    fn no_match_filters_entry_out() {
        let cands = vec![Candidate::new("abc", 0.0)];
        let hits = Matcher::new(&cands).query("xyz", 0);
        assert!(hits.is_empty());
    }

    #[test]
    fn perfect_consecutive_match_scores_expected_value() {
        // text="abc", query="abc", base=0
        //   pos 0: +1 + 1 (boundary)              = 2
        //   pos 1: +1 + 0 + 2/sqrt(1) = +1 + 2    = 3
        //   pos 2: +1 + 0 + 2/sqrt(1) = +1 + 2    = 3
        // subtotal: 8
        // density: 3/3 = 1.0
        // length:  10/13
        // final:   8 * 1 * 10/13 ≈ 6.153846
        let cands = vec![Candidate::new("abc", 0.0)];
        let hits = Matcher::new(&cands).query("abc", 0);
        assert_eq!(hits.len(), 1);
        approx(hits[0].score, 8.0 * (10.0 / 13.0));
        assert_eq!(hits[0].positions, vec![0, 1, 2]);
    }

    #[test]
    fn base_score_is_purely_additive() {
        let cands = vec![Candidate::new("abc", 5.0)];
        let hits = Matcher::new(&cands).query("abc", 0);
        // Same as above plus 5.0 base.
        approx(hits[0].score, 8.0 * (10.0 / 13.0) + 5.0);
    }

    #[test]
    fn case_insensitive_matching() {
        let cands = vec![Candidate::new("ABC", 0.0)];
        let hits = Matcher::new(&cands).query("abc", 0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].positions, vec![0, 1, 2]);
    }

    #[test]
    fn word_boundary_after_hyphen_is_rewarded() {
        // Score "redis" subsequence in two names; the one with `redis` at a
        // word boundary should score higher than the same length name where
        // the match starts mid-word.
        let cands = vec![
            Candidate::new("foo-redis-bar", 0.0),
            Candidate::new("fooredisbar00", 0.0), // same total length
        ];
        let hits = Matcher::new(&cands).query("redis", 0);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn limit_caps_results() {
        let cands: Vec<Candidate> = (0..10)
            .map(|i| Candidate::new(format!("entry-{i}"), i as f64))
            .collect();
        let hits = Matcher::new(&cands).query("", 3);
        assert_eq!(hits.len(), 3);
        // Higher base scores come first.
        assert_eq!(hits[0].index, 9);
        assert_eq!(hits[1].index, 8);
        assert_eq!(hits[2].index, 7);
    }
}
