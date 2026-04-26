//! Required-literal extraction for `xray_grep` `lineRegex` mode (AC-4).
//!
//! Wraps `regex_syntax::hir::literal::Extractor` so that callers can answer
//! one question: *"is there a finite set of substrings such that every match
//! of this regex is guaranteed to contain at least one of them?"* When the
//! answer is yes, the trigram index can prefilter candidate files before the
//! per-line regex scan runs (see `compute_literal_prefilter` in `grep.rs`).
//!
//! The extractor is conservative on purpose: any uncertainty (`Seq` is
//! infinite, marked inexact, contains a literal shorter than the trigram
//! width, or `regex-syntax` cannot parse the pattern at all) collapses to
//! "fall back to full scan". A false positive here would silently drop
//! matches from search results — `usable: false` is always the safe default.
//!
//! Literals are lowercased to match the trigram index, whose tokens are
//! lowercased at index time. This loses no information for the prefilter
//! step because the per-line regex is still the final arbiter on each
//! candidate file.

use regex_syntax::hir::literal::Extractor;
use regex_syntax::ParserBuilder;

/// Minimum literal length that yields a usable trigram. Literals shorter
/// than this cannot be looked up in the trigram index, so a single short
/// literal in the extracted Seq disqualifies the whole pattern (we cannot
/// safely prune candidate files when one branch is unprefilterable).
pub(super) const MIN_LITERAL_LEN: usize = 3;

/// Result of attempting to extract required substring literals from a regex.
///
/// Returned by [`extract_required_literals`]. The caller pairs the
/// `usable` flag with `literals` to decide whether to apply the trigram
/// prefilter for this pattern or fall back to full-scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExtractedLiterals {
    /// Lowercased required-substring literals. When `usable` is true, every
    /// match of the source regex is guaranteed to contain at least one of
    /// these as a substring (per `Seq` semantics from `regex-syntax`).
    pub literals: Vec<String>,
    /// True only when the extractor produced a finite, non-empty literal
    /// set AND every literal is at least [`MIN_LITERAL_LEN`] characters long
    /// (Unicode scalar values, NOT bytes — the trigram pipeline slides over
    /// chars for non-ASCII input, so a 2-byte ASCII fragment and a 2-char
    /// Cyrillic fragment must be treated identically).
    /// False signals "no safe prefilter possible — caller must fall back
    /// to the current full-scan path".
    pub usable: bool,
}

impl ExtractedLiterals {
    fn unusable() -> Self {
        Self { literals: Vec::new(), usable: false }
    }
}

/// Attempt to extract a set of required substring literals from `pattern`.
///
/// Returns:
/// - `None` when `regex-syntax` cannot parse the pattern at all (rare —
///   the wider `regex` crate occasionally accepts inputs `regex-syntax`
///   rejects, e.g. some exotic Unicode classes). Callers treat `None`
///   identically to `Some(unusable)` and fall back to full scan.
/// - `Some(ExtractedLiterals { usable: false, .. })` when the regex has
///   no provable required substring (`.* `, `\w+`, lookarounds, etc.) or
///   when at least one extracted literal is shorter than
///   [`MIN_LITERAL_LEN`] characters (would not produce a trigram).
/// - `Some(ExtractedLiterals { usable: true, literals })` with each
///   literal lowercased to match the trigram index's lowercased tokens.
///
/// **Correctness invariant:** if `usable` is true, then for every input
/// string `s` matched by the source regex, `s.to_lowercase().contains(L)`
/// holds for at least one literal `L` in `literals`. Violating this would
/// cause silent false negatives in `xray_grep` results.
pub(super) fn extract_required_literals(pattern: &str) -> Option<ExtractedLiterals> {
    // `ParserBuilder::default()` mirrors `regex` crate defaults (Unicode
    // on, case-insensitive only when the pattern carries `(?i)`). We do
    // NOT pre-set `case_insensitive(true)` — that would change semantics
    // for patterns the user expects to be case-sensitive.
    let hir = ParserBuilder::new().build().parse(pattern).ok()?;

    let seq = Extractor::new().extract(&hir);

    // `is_finite() == false` means the extractor gave up enumerating
    // (e.g. `.*`). `is_empty()` means there is no required literal at
    // all. Note: we deliberately do NOT reject on `is_inexact()`.
    // `Seq::is_exact()` means the literals are the *complete* set of
    // possible matches (true only for trivial patterns like `(foo|bar)`).
    // For prefilter purposes the relevant property is the weaker
    // "every match contains at least one literal as substring" — which
    // a finite, non-empty `Kind::Prefix` Seq guarantees by construction.
    if !seq.is_finite() || seq.is_empty() {
        return Some(ExtractedLiterals::unusable());
    }

    let Some(literals) = seq.literals() else {
        return Some(ExtractedLiterals::unusable());
    };

    let mut out = Vec::with_capacity(literals.len());
    for lit in literals {
        // `Literal::as_bytes()` may contain non-UTF-8 bytes if the source
        // regex was compiled in `bytes` mode. xray's `lineRegex` always
        // operates on Unicode text, so a non-UTF-8 literal is anomalous —
        // treat it the same as an unprefilterable pattern (safe default).
        let s = match std::str::from_utf8(lit.as_bytes()) {
            Ok(s) => s,
            Err(_) => return Some(ExtractedLiterals::unusable()),
        };
        if s.chars().count() < MIN_LITERAL_LEN {
            // Length is measured in characters (Unicode scalar values), not
            // bytes — `generate_trigrams` slides over chars for non-ASCII
            // input, so a 2-char Cyrillic literal like "ЯБ" produces zero
            // trigrams even though it is 4 bytes. Using `.len()` here would
            // silently mark such a literal usable and produce false negatives.
            //
            // Even one too-short literal disqualifies the whole pattern:
            // we cannot prefilter on the long literals alone because the
            // short branch could match files containing none of them.
            return Some(ExtractedLiterals::unusable());
        }
        // Lowercase to match the trigram index's lowercased tokens. The
        // trigram step is over-approximate anyway (it returns candidate
        // tokens, not exact matches), so case-folding here is safe — the
        // per-line regex on each candidate file is the final arbiter.
        out.push(s.to_lowercase());
    }

    // Deduplicate identical literals (Extractor can produce duplicates
    // for patterns like `(foo|foo|bar)`). Sort first so dedup is cheap
    // and the literal order is deterministic for snapshot tests.
    out.sort();
    out.dedup();

    if out.is_empty() {
        return Some(ExtractedLiterals::unusable());
    }

    Some(ExtractedLiterals { literals: out, usable: true })
}

#[cfg(test)]
#[path = "grep_literal_extract_tests.rs"]
mod tests;
