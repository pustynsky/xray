use std::collections::HashMap;
use std::fmt;

use serde_json::{json, Value};

use crate::{ContentIndex, Posting};

pub(crate) const TOKEN_REGEX_PREVIEW_MAX: usize = 20;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TokenSearchTelemetry {
    pub(crate) posting_lists_visited: usize,
    pub(crate) postings_checked: usize,
    pub(crate) postings_in_scope: usize,
}

#[derive(Debug)]
pub(crate) struct RegexExpansionError {
    pub(crate) pattern: String,
    pub(crate) source: regex::Error,
}

impl fmt::Display for RegexExpansionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Invalid regex '{}': {}", self.pattern, self.source)
    }
}

impl std::error::Error for RegexExpansionError {}

#[derive(Debug, Clone)]
pub(crate) struct RegexExpansion {
    pub(crate) patterns: usize,
    pub(crate) tokens_examined: usize,
    pub(crate) expanded_tokens: Vec<String>,
    pub(crate) pattern_match_counts: Vec<usize>,
    pub(crate) posting_telemetry: TokenSearchTelemetry,
}

impl RegexExpansion {
    pub(crate) fn to_json(&self, count_only: bool) -> Value {
        let mut value = json!({
            "patterns": self.patterns,
            "tokensExamined": self.tokens_examined,
            "matchedTokenCount": self.expanded_tokens.len(),
            "postingListsVisited": self.posting_telemetry.posting_lists_visited,
            "postingsChecked": self.posting_telemetry.postings_checked,
            "postingsInScope": self.posting_telemetry.postings_in_scope,
        });
        if !count_only {
            let preview: Vec<&str> = self.expanded_tokens.iter()
                .take(TOKEN_REGEX_PREVIEW_MAX)
                .map(String::as_str)
                .collect();
            value["matchedTokenPreview"] = json!(preview);
            // This contract is a numeric hidden-token count, not a boolean.
            value["previewTruncated"] = json!(
                self.expanded_tokens.len().saturating_sub(TOKEN_REGEX_PREVIEW_MAX)
            );
        }
        value
    }
}

fn expand_regex_terms_inner(
    raw_terms: &[String],
    index_keys: &HashMap<String, Vec<Posting>>,
    deduplicate: bool,
) -> Result<RegexExpansion, RegexExpansionError> {
    let mut expanded = Vec::new();
    let mut pattern_match_counts = Vec::with_capacity(raw_terms.len());
    let mut tokens_examined = 0usize;
    for pattern in raw_terms {
        let regex = regex::Regex::new(&format!("(?i)^{}$", pattern))
            .map_err(|source| RegexExpansionError {
                pattern: pattern.clone(),
                source,
            })?;
        let mut pattern_matches = 0usize;
        for token in index_keys.keys() {
            tokens_examined = tokens_examined.saturating_add(1);
            if regex.is_match(token) {
                pattern_matches = pattern_matches.saturating_add(1);
                expanded.push(token.clone());
            }
        }
        pattern_match_counts.push(pattern_matches);
    }

    if deduplicate {
        // MCP expansion is sorted for deterministic previews, then deduplicated
        // so overlapping patterns do not make one token contribute twice.
        expanded.sort();
        expanded.dedup();
    }
    Ok(RegexExpansion {
        patterns: raw_terms.len(),
        tokens_examined,
        expanded_tokens: expanded,
        pattern_match_counts,
        posting_telemetry: TokenSearchTelemetry::default(),
    })
}

pub(crate) fn expand_regex_terms(
    raw_terms: &[String],
    index: &ContentIndex,
) -> Result<RegexExpansion, RegexExpansionError> {
    expand_regex_terms_inner(raw_terms, &index.index, true)
}

pub(crate) fn expand_regex_terms_preserving_duplicates(
    raw_terms: &[String],
    index_keys: &HashMap<String, Vec<Posting>>,
) -> Result<RegexExpansion, RegexExpansionError> {
    expand_regex_terms_inner(raw_terms, index_keys, false)
}

