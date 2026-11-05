use crate::core::{AppError, AppResult};
use crate::utilities::strings::{
    extract_title_from_content, extract_title_from_filename, sanitize_fts_query,
};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use rusqlite::params;
use serde::Serialize;
use std::cmp::Ordering;

#[derive(Serialize)]
pub struct NoteMetadata {
    pub filename: String,
    pub modified: i64,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub filename: String,
    pub title: String,
    pub score: u32,
    match_type: MatchType,
    pub modified: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MatchType {
    ExactTitle = 3,
    PrefixTitle = 2,
    FuzzyTitle = 1,
    Content = 0,
}

#[derive(Debug, Clone)]
struct SearchCandidate {
    filename: String,
    title: String,
    content: String,
    modified: i64,
}

pub struct HybridSearcher {
    matcher: Matcher,
    /// Reused across candidates so scoring does not allocate a lowercased copy
    /// of every note body on every keystroke.
    lowercase_buf: String,
}

/// How many rows FTS5 hands to the fuzzy ranker.
///
/// FTS5 only generates candidates here; the ranking that decides what the user
/// sees happens afterwards in Rust. A fixed cap of 500 could therefore hide a
/// note that would have scored well, whenever BM25 ranked it below the cap and
/// the configured result count was larger. Never fetch fewer candidates than
/// the number of results asked for.
const MIN_CANDIDATES: usize = 500;

pub(crate) fn candidate_limit(max_results: usize) -> usize {
    max_results.max(MIN_CANDIDATES)
}

/// Lowercases ASCII `text` into `buf`, replacing whatever it held.
///
/// Must stay equivalent to `text.to_lowercase()`, since the query is lowercased
/// that way and any divergence stops content matching a query that should find
/// it. Only ASCII goes through here: Unicode lowercasing is context-sensitive —
/// it maps a final Greek sigma to `\u{3C2}` where a per-character mapping gives
/// `\u{3C3}` — so non-ASCII keeps `to_lowercase` instead.
pub(crate) fn lowercase_ascii_into(buf: &mut String, text: &str) {
    debug_assert!(text.is_ascii(), "caller must check is_ascii first");
    buf.clear();
    buf.push_str(text);
    buf.make_ascii_lowercase();
}

/// Builds the FTS5 MATCH pattern for a sanitised query.
///
/// Every term is quoted. `sanitize_fts_query` keeps `-`, which FTS5 reads as an
/// operator, so `foo-bar*` parses as a column filter and the whole query errors
/// with "no such column". Quoting neutralises it, and for terms without `-` the
/// quoted and unquoted forms match identically.
///
/// Terms are joined with AND: a note has to contain every word, in any order,
/// and typing another word narrows the list. Joining with OR made each extra
/// word widen the candidate set instead.
pub(crate) fn build_fts_pattern(sanitized_query: &str) -> String {
    let quoted = |word: &str| format!("\"{}\"*", word);

    if sanitized_query.contains(' ') {
        sanitized_query
            .split_whitespace()
            .filter(|word| !word.trim().is_empty())
            .map(quoted)
            .collect::<Vec<_>>()
            .join(" AND ")
    } else {
        quoted(sanitized_query)
    }
}

/// The lowercase words a note has to contain to match `query`.
///
/// Built from the sanitised query so it agrees with what FTS5 was asked for:
/// punctuation FTS5 never sees, and the bare words AND/OR/NOT/NEAR/MATCH that
/// the sanitiser removes as operators, are not required to appear in the note.
pub(crate) fn query_words(query: &str) -> Vec<String> {
    sanitize_fts_query(query)
        .to_lowercase()
        .split_whitespace()
        .map(|word| word.trim_matches('*'))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// A note's file name without its note extension: `a-b.md` is `a-b`.
fn without_note_extension(filename_lower: &str) -> &str {
    [".markdown", ".md", ".txt"]
        .iter()
        .find_map(|ext| filename_lower.strip_suffix(ext))
        .unwrap_or(filename_lower)
}

/// Content score when every word appears in the title, the file name or the
/// body, otherwise `None`. Everything passed in is already lowercase.
///
/// Substring match, as in nvALT: `ount` finds `account`. A word found only in
/// the title or file name still counts once, so a note called "Acme" whose body
/// never repeats the name is found by "acme invoice".
fn word_coverage_score(
    words: &[String],
    title_lower: &str,
    filename_lower: &str,
    content_lower: &str,
) -> Option<u32> {
    let name_lower = without_note_extension(filename_lower);
    let mut score = 50u32;

    for word in words {
        let in_body = content_lower.matches(word.as_str()).count() as u32;
        let in_name = title_lower.contains(word.as_str()) || name_lower.contains(word.as_str());

        if in_body == 0 && !in_name {
            return None;
        }
        score = score.saturating_add(in_body.max(1).saturating_mul(10));
    }

    Some(score)
}

impl HybridSearcher {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let matcher = Matcher::new(Config::DEFAULT);
        Ok(Self {
            matcher,
            lowercase_buf: String::new(),
        })
    }

    pub fn search(
        &mut self,
        app_state: &crate::core::state::AppState,
        query: &str,
        max_results: usize,
    ) -> AppResult<Vec<NoteMetadata>> {
        if query.trim().is_empty() {
            return self.get_recent_notes(app_state, max_results);
        }

        let candidates = self.get_candidates_from_sqlite(app_state, query, max_results)?;
        let mut results = Vec::new();

        for candidate in candidates {
            if let Some(result) = self.score_candidate(&candidate, query) {
                results.push(result);
            }
        }

        results.sort_by(|a, b| self.compare_results(a, b));
        results.truncate(max_results);

        Ok(results
            .into_iter()
            .map(|r| NoteMetadata {
                filename: r.filename,
                modified: r.modified,
            })
            .collect())
    }

    fn get_candidates_from_sqlite(
        &self,
        app_state: &crate::core::state::AppState,
        query: &str,
        max_results: usize,
    ) -> AppResult<Vec<SearchCandidate>> {
        let sanitized_query = sanitize_fts_query(query);

        if sanitized_query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let fts_pattern = build_fts_pattern(&sanitized_query);

        crate::database::with_db(app_state, |conn| {
            let mut stmt = conn.prepare(
                "SELECT filename, content, modified FROM notes
                     WHERE notes MATCH ?
                     ORDER BY rank
                     LIMIT ?",
            )?;

            let rows =
                stmt.query_map(params![fts_pattern, candidate_limit(max_results)], |row| {
                    let filename: String = row.get(0)?;
                    let content: String = row.get(1)?;
                    let modified: i64 = row.get(2)?;

                    let title = extract_title_from_content(&content)
                        .unwrap_or_else(|| extract_title_from_filename(&filename));

                    Ok(SearchCandidate {
                        filename,
                        title,
                        content,
                        modified,
                    })
                })?;

            let candidates = rows.collect::<Result<Vec<_>, _>>()?;
            Ok(candidates)
        })
    }

    fn score_candidate(
        &mut self,
        candidate: &SearchCandidate,
        query: &str,
    ) -> Option<SearchResult> {
        let words = query_words(query);
        if words.is_empty() {
            return None;
        }

        let query_lower = query.trim().to_lowercase();
        let title_lower = candidate.title.to_lowercase();
        let filename_lower = candidate.filename.to_lowercase();

        // The note body is lowercased once per candidate: it decides whether the
        // note matches at all, whatever its title looks like.
        //
        // Unicode lowercasing is context-sensitive, so non-ASCII keeps one
        // `to_lowercase`. ASCII lowercases in place into a buffer reused across
        // candidates; `to_lowercase()` there allocated a fresh copy of every
        // body on every keystroke. The buffer is taken out of `self` so the
        // scorer below can still borrow `self` mutably.
        let mut buf = std::mem::take(&mut self.lowercase_buf);
        let unicode_lowered;
        let content_lower: &str = if candidate.content.is_ascii() {
            lowercase_ascii_into(&mut buf, &candidate.content);
            &buf
        } else {
            unicode_lowered = candidate.content.to_lowercase();
            &unicode_lowered
        };

        let result = self.score_lowered_candidate(
            candidate,
            &query_lower,
            &words,
            &title_lower,
            &filename_lower,
            content_lower,
        );
        self.lowercase_buf = buf;
        result
    }

    fn score_lowered_candidate(
        &mut self,
        candidate: &SearchCandidate,
        query_lower: &str,
        words: &[String],
        title_lower: &str,
        filename_lower: &str,
        content_lower: &str,
    ) -> Option<SearchResult> {
        // Every word has to be somewhere in the note. Without this gate the
        // title rules below, which are fuzzy, could surface a note that does
        // not contain the query at all.
        let content_score = word_coverage_score(words, title_lower, filename_lower, content_lower)?;

        let (score, match_type) = self
            .score_title_match(title_lower, filename_lower, query_lower, words)
            .unwrap_or((content_score, MatchType::Content));

        Some(SearchResult {
            filename: candidate.filename.clone(),
            title: candidate.title.clone(),
            score,
            match_type,
            modified: candidate.modified,
        })
    }

    fn score_title_match(
        &mut self,
        title_lower: &str,
        filename_lower: &str,
        query_lower: &str,
        words: &[String],
    ) -> Option<(u32, MatchType)> {
        for (text, boost) in [(title_lower, 100), (filename_lower, 50)] {
            if text == query_lower {
                return Some((1000 + boost, MatchType::ExactTitle));
            }

            if text.starts_with(query_lower) {
                return Some((800 + boost, MatchType::PrefixTitle));
            }

            let text_words: Vec<&str> = text
                .split(|c: char| "_-.,+=;: ".contains(c) || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .collect();

            if text_words.iter().any(|word| word.starts_with(query_lower)) {
                return Some((700 + boost, MatchType::PrefixTitle));
            }

            // A multi-word query starts every word of the title in any order:
            // "inv acme" finds "Acme Invoice Q3".
            if words.len() > 1
                && words.iter().all(|query_word| {
                    text_words
                        .iter()
                        .any(|word| word.starts_with(query_word.as_str()))
                })
            {
                return Some((650 + boost, MatchType::PrefixTitle));
            }

            if let Some(score) = self.fuzzy_match(text, query_lower) {
                if score > 50 {
                    return Some((score + boost, MatchType::FuzzyTitle));
                }
            }
        }

        None
    }

    /// Fuzzy score of `needle` within `text`.
    ///
    /// `nucleo_matcher::Matcher::fuzzy_match` takes the haystack first and the
    /// needle second. This used to pass them the other way round, which asks
    /// whether the *note* is a subsequence of the *query*, so a fuzzy match
    /// practically never happened.
    fn fuzzy_match(&mut self, text: &str, query: &str) -> Option<u32> {
        let mut haystack_buf = Vec::new();
        let mut needle_buf = Vec::new();
        let haystack = Utf32Str::new(text, &mut haystack_buf);
        let needle = Utf32Str::new(query, &mut needle_buf);
        self.matcher
            .fuzzy_match(haystack, needle)
            .map(|score| score as u32)
    }

    fn compare_results(&self, a: &SearchResult, b: &SearchResult) -> Ordering {
        b.match_type
            .cmp(&a.match_type)
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| b.modified.cmp(&a.modified))
            .then_with(|| a.title.cmp(&b.title))
    }

    fn get_recent_notes(
        &self,
        app_state: &crate::core::state::AppState,
        max_results: usize,
    ) -> AppResult<Vec<NoteMetadata>> {
        crate::database::with_db(app_state, |conn| {
            let mut stmt = conn
                .prepare("SELECT filename, modified FROM notes ORDER BY modified DESC LIMIT ?")?;

            let rows = stmt.query_map([max_results], |row| {
                Ok(NoteMetadata {
                    filename: row.get(0)?,
                    modified: row.get(1)?,
                })
            })?;

            let notes = rows.collect::<Result<Vec<_>, _>>()?;
            Ok(notes)
        })
    }
}

pub fn search_notes_hybrid(
    app_state: &crate::core::state::AppState,
    query: &str,
    max_results: usize,
) -> AppResult<Vec<NoteMetadata>> {
    let mut searcher =
        HybridSearcher::new().map_err(|e| AppError::DatabaseConnection(e.to_string()))?;
    searcher.search(app_state, query, max_results)
}

#[cfg(test)]
mod scoring_tests {
    use super::*;

    fn candidate(filename: &str, content: &str) -> SearchCandidate {
        SearchCandidate {
            filename: filename.to_string(),
            title: extract_title_from_content(content)
                .unwrap_or_else(|| extract_title_from_filename(filename)),
            content: content.to_string(),
            modified: 0,
        }
    }

    fn score(candidate: &SearchCandidate, query: &str) -> Option<SearchResult> {
        HybridSearcher::new()
            .expect("matcher should build")
            .score_candidate(candidate, query)
    }

    const FAR_APART: &str = "Acme kickoff\n\nLots of unrelated text about the project plan and \
                             the roadmap for next quarter.\n\nMuch later: the invoice was sent.";

    #[test]
    fn fuzzy_match_takes_the_haystack_first() {
        let mut searcher = HybridSearcher::new().unwrap();

        // The needle is a subsequence of the haystack, not the other way round.
        assert!(searcher.fuzzy_match("account balance", "ount").is_some());
        assert!(searcher.fuzzy_match("ount", "account balance").is_none());
    }

    #[test]
    fn every_word_must_be_present_but_not_next_to_each_other() {
        let note = candidate("acme.md", FAR_APART);

        let hit = score(&note, "acme invoice").expect("both words are in the note");
        assert_eq!(hit.match_type, MatchType::Content);
    }

    #[test]
    fn word_order_in_the_query_does_not_matter() {
        let note = candidate("acme.md", FAR_APART);

        assert!(score(&note, "invoice acme").is_some());
    }

    #[test]
    fn a_missing_word_rules_the_note_out() {
        let note = candidate("acme.md", "Acme kickoff\n\nRoadmap and pricing only.");

        assert!(score(&note, "acme invoice").is_none());
    }

    #[test]
    fn a_word_that_is_not_in_the_note_never_matches_by_fuzziness() {
        // z, e, b, r, a all occur in order, so a fuzzy matcher would accept it.
        let note = candidate("plain.md", "the lazy dog zips by every bright rainbow area");

        assert!(score(&note, "zebra").is_none());
    }

    #[test]
    fn words_match_inside_other_words() {
        let note = candidate("bank.md", "Bank\n\naccount balance");

        assert!(score(&note, "ount").is_some());
    }

    #[test]
    fn a_word_found_only_in_the_file_name_counts() {
        let note = candidate("acme.md", "Invoice sent on Friday");

        assert!(score(&note, "acme invoice").is_some());
    }

    #[test]
    fn the_file_extension_is_not_part_of_the_name() {
        let note = candidate("acme.md", "Acme\n\nroadmap");

        assert!(score(&note, "md").is_none());
        assert_eq!(without_note_extension("a-b.md"), "a-b");
        assert_eq!(without_note_extension("a.markdown"), "a");
        assert_eq!(without_note_extension("a.txt"), "a");
        assert_eq!(without_note_extension("a.rs"), "a.rs");
    }

    #[test]
    fn an_exact_title_still_outranks_a_content_hit() {
        let titled = candidate("acme.md", "Acme");
        let body = candidate("other.md", "Other\n\nmentions acme in passing");

        let exact = score(&titled, "acme").unwrap();
        let content = score(&body, "acme").unwrap();

        assert_eq!(exact.match_type, MatchType::ExactTitle);
        assert_eq!(content.match_type, MatchType::Content);
        assert!(exact.match_type > content.match_type);
    }

    #[test]
    fn every_word_can_start_a_word_of_the_title() {
        let note = candidate("q3.md", "Acme Invoice Q3\n\nbody");

        let hit = score(&note, "inv acme").expect("both words start title words");
        assert_eq!(hit.match_type, MatchType::PrefixTitle);
        assert_eq!(hit.score, 750);
    }

    #[test]
    fn query_words_follow_the_sanitised_query() {
        assert_eq!(query_words("Rock and roll"), vec!["rock", "roll"]);
        assert_eq!(query_words("foo* bar,"), vec!["foo", "bar"]);
        assert_eq!(query_words("  Acme   Invoice "), vec!["acme", "invoice"]);
        assert!(query_words("AND OR").is_empty());
    }
}
