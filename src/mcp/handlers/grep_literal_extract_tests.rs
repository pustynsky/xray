//! Unit tests for `extract_required_literals` (AC-4 literal extraction).
//!
//! These tests pin the contract that `handle_line_regex_search` relies on
//! when deciding whether to apply a trigram prefilter. **Each `usable: true`
//! case is also a correctness assertion**: the listed literals MUST cover
//! every possible match of the source regex (no false negatives possible).
//!
//! When `regex-syntax`'s extractor changes behaviour across versions and a
//! test here breaks, the fix is *not* to weaken the assertion — re-evaluate
//! whether the new extractor output still preserves the correctness
//! invariant before updating the expected literals.

use super::{extract_required_literals, ExtractedLiterals, MIN_LITERAL_LEN};

fn usable(literals: &[&str]) -> ExtractedLiterals {
    let mut owned: Vec<String> = literals.iter().map(|s| s.to_string()).collect();
    owned.sort();
    owned.dedup();
    ExtractedLiterals { literals: owned, usable: true }
}

fn unusable() -> ExtractedLiterals {
    ExtractedLiterals { literals: Vec::new(), usable: false }
}

#[test]
fn extracts_simple_prefix_literal() {
    // `App\s*=\s*\d+` — the canonical AC-4 motivating case. Extractor
    // sees "App" as the required prefix; everything after is variable.
    let got = extract_required_literals(r"App\s*=\s*\d+").unwrap();
    assert_eq!(got, usable(&["app"]));
}

#[test]
fn extracts_literal_with_internal_whitespace() {
    // `^pub fn` — anchor + literal containing space. Anchors do not
    // change required-literal semantics; the literal is still "pub fn".
    let got = extract_required_literals(r"^pub fn").unwrap();
    assert_eq!(got, usable(&["pub fn"]));
}

#[test]
fn extracts_alternation_branches() {
    // `(foo|bar)` — both branches must be present in the literal set;
    // neither alone covers all matches.
    let got = extract_required_literals(r"(foo|bar)").unwrap();
    assert_eq!(got, usable(&["bar", "foo"]));
}

#[test]
fn case_insensitive_flag_lowercases_literal() {
    // `(?i)App` — case-insensitive flag. The extractor expands this to
    // multiple cased literals; after lowercasing + dedup we should get a
    // single "app". Whatever the extractor returns, the contract is
    // "lowercased and dedup'd" — the test pins that contract.
    let got = extract_required_literals(r"(?i)Application").unwrap();
    assert!(got.usable, "case-insensitive literal must remain usable");
    assert!(
        got.literals.iter().all(|s| s.chars().all(|c| !c.is_uppercase())),
        "all literals must be lowercased, got: {:?}",
        got.literals
    );
    // Each lowercased literal must be a prefix of "application" -- this is
    // the correctness invariant: regardless of how the extractor truncates
    // the case-expansion combinatorics, every match of `(?i)Application`
    // starts with one of the returned (lowercased) prefixes, so the
    // trigram prefilter on those prefixes will not cause false negatives.
    assert!(
        got.literals.iter().all(|s| "application".starts_with(s.as_str())),
        "every literal must be a prefix of 'application'; got: {:?}",
        got.literals
    );
}

#[test]
fn rejects_unbounded_quantifier_only() {
    // `.*` -- matches anything. No required literal. Must fall back.
    let got = extract_required_literals(r".*").unwrap();
    assert_eq!(got, unusable());
}

#[test]
fn rejects_word_quantifier_only() {
    // `\w+` — no fixed substring. Must fall back.
    let got = extract_required_literals(r"\w+").unwrap();
    assert_eq!(got, unusable());
}

#[test]
fn rejects_pattern_with_no_required_prefix() {
    // `\w+_\d+` — alternation of character classes; no required literal.
    let got = extract_required_literals(r"\w+_\d+").unwrap();
    assert_eq!(got, unusable());
}

#[test]
fn rejects_short_literal_below_trigram_width() {
    // `^=` — a literal of length 1, well below MIN_LITERAL_LEN. Even
    // though "=" is a perfectly valid required literal, we cannot
    // trigram-filter on it, so the pattern is unusable for prefilter.
    let got = extract_required_literals(r"^=").unwrap();
    assert_eq!(got, unusable());
    // Sanity: confirm the constant is still 3 (trigram width).
    assert_eq!(MIN_LITERAL_LEN, 3);
}

#[test]
fn rejects_alternation_when_one_branch_too_short() {
    // `(foo|=)` — one branch is fine ("foo"), the other ("=") is too
    // short. We cannot drop the short branch (it would cause false
    // negatives on inputs that match only via "="), so the entire
    // pattern is unprefilterable.
    let got = extract_required_literals(r"(foo|=)").unwrap();
    assert_eq!(got, unusable());
}

#[test]
fn anchored_literal_still_extracted() {
    // `^foo$` — both anchors. Required literal is still "foo".
    let got = extract_required_literals(r"^foo$").unwrap();
    assert_eq!(got, usable(&["foo"]));
}

#[test]
fn deduplicates_repeated_literal() {
    // `(foo|foo|foo)` — extractor may produce duplicates; we dedup.
    let got = extract_required_literals(r"(foo|foo|foo)").unwrap();
    assert_eq!(got, usable(&["foo"]));
}

#[test]
fn handles_unicode_literal() {
    // Cyrillic literal — must stay usable, lowercased correctly.
    // `Привет` lowercased is `привет`; both are 12 bytes (>= 3).
    let got = extract_required_literals(r"Привет").unwrap();
    assert_eq!(got, usable(&["привет"]));
}

#[test]
fn rejects_two_char_cyrillic_literal_as_too_short() {
    // Regression for AC-4 review finding: `ЯБ` is 4 bytes but only 2
    // CHARACTERS, and `generate_trigrams` slides over chars for non-ASCII
    // input (no trigrams produced for <3 chars). If the extractor used
    // `len()` (bytes), it would mark this usable, the trigram lookup
    // would silently return an empty file set, and `lineRegex=ЯБ` would
    // produce false negatives. Char-count gating prevents that.
    let got = extract_required_literals(r"ЯБ").unwrap();
    assert!(!got.usable, "2-char Cyrillic literal must be unprefilterable");
}

#[test]
fn accepts_three_char_cyrillic_literal() {
    // Companion to the rejection test: 3 chars (6 bytes) IS the trigram
    // floor; lookup will produce one trigram and the prefilter is safe.
    let got = extract_required_literals(r"ЯБВ").unwrap();
    assert_eq!(got, usable(&["ябв"]));
}


#[test]
fn rejects_lookahead_only_pattern() {
    // `(?=foo)` — lookaround. `regex-syntax` rejects pure lookarounds
    // in standard mode (returns parse error), so we expect `None`.
    // The caller treats `None` identically to `Some(unusable)`.
    let got = extract_required_literals(r"(?=foo)");
    assert!(
        got.is_none() || got == Some(unusable()),
        "lookaround must either fail to parse (None) or be marked unusable, got: {:?}",
        got
    );
}

#[test]
fn invalid_regex_returns_none_gracefully() {
    // Unbalanced bracket — regex-syntax parse error. Must return None,
    // not panic. Caller falls back to full scan (and the regex crate
    // itself will surface the error to the user via existing path).
    let got = extract_required_literals(r"[unbalanced");
    assert_eq!(got, None);
}

#[test]
fn empty_pattern_falls_back() {
    // Empty pattern parses to empty Hir → empty Seq → unusable.
    let got = extract_required_literals(r"").unwrap();
    assert_eq!(got, unusable());
}

#[test]
fn char_class_at_start_extracts_when_branches_long_enough() {
    // `[Aa]pp` — character class at start is equivalent to alternation
    // `(?:Aapp|app)` after lowercasing. Both branches "app" after
    // lowercasing+dedup → single literal "app".
    let got = extract_required_literals(r"[Aa]pplication").unwrap();
    assert!(got.usable, "char-class start must produce usable extraction");
    assert!(
        got.literals.iter().any(|s| s == "application"),
        "expected 'application' literal, got: {:?}",
        got.literals
    );
}

#[test]
fn rejects_pattern_starting_with_optional_group() {
    // `(?:foo)?bar` — leading group is optional, so prefix literal
    // extraction yields just "bar" (not "foobar"). Still usable.
    let got = extract_required_literals(r"(?:foo)?bar").unwrap();
    assert!(got.usable);
    assert!(
        got.literals.iter().any(|s| s == "bar"),
        "expected at least 'bar' literal, got: {:?}",
        got.literals
    );
}

#[test]
fn long_alternation_stays_usable_or_falls_back_safely() {
    // 26-branch alternation — extractor has internal caps. We only
    // assert the result is *safe* (either usable with all branches OR
    // explicitly unusable) — never a partial usable set, which would be
    // a correctness bug.
    let pat = r"(alpha|bravo|charlie|delta|echo|foxtrot|golf|hotel|india|juliet|kilo|lima|mike|november|oscar|papa|quebec|romeo|sierra|tango|uniform|victor|whiskey|xray|yankee|zulu)";
    let got = extract_required_literals(pat).unwrap();
    if got.usable {
        // If extractor kept the full set, all 26 branches must be present.
        assert_eq!(
            got.literals.len(),
            26,
            "usable result must list all branches; truncation would cause false negatives"
        );
    }
    // Otherwise unusable is fine — we just fall back to full scan.
}

#[test]
fn rejects_pattern_with_only_anchors() {
    // `^$` — empty line matcher. No literal at all.
    let got = extract_required_literals(r"^$").unwrap();
    assert_eq!(got, unusable());
}

#[test]
fn extracts_concatenation_of_literals() {
    // `foo[a-z]+bar` — `Kind::Prefix` extractor only sees the prefix
    // "foo" (anything past the variable middle is not a *required*
    // prefix). We accept whatever the extractor returns as long as it
    // is a non-empty subset of the required substrings; pin "foo" as
    // the bare-minimum expectation.
    let got = extract_required_literals(r"foo[a-z]+bar").unwrap();
    assert!(got.usable, "fixed prefix should yield usable extraction");
    assert!(
        got.literals.iter().any(|s| s == "foo"),
        "expected 'foo' literal in {:?}",
        got.literals
    );
}
