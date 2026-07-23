use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::Posting;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegexExpansionDedup {
    DeduplicateSorted,
    PreservePatternDuplicates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenRegexExpansionStrategy {
    EmptyScope,
    GlobalVocabulary,
}

impl TokenRegexExpansionStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::EmptyScope => "emptyScope",
            Self::GlobalVocabulary => "globalVocabulary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenRegexStrategyReason {
    EmptyResolvedScope,
    GlobalVocabularyBaseline,
}

impl TokenRegexStrategyReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::EmptyResolvedScope => "emptyResolvedScope",
            Self::GlobalVocabularyBaseline => "globalVocabularyBaseline",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenRegexAccountingScope {
    None,
    GlobalVocabulary,
}

impl TokenRegexAccountingScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::GlobalVocabulary => "globalVocabulary",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RegexExpansionTimings {
    compile: Duration,
    plan: Duration,
    universe_build: Duration,
    scan_collect: Duration,
    sort_dedup: Duration,
    expansion_total: Duration,
    posting_score: Duration,
}

#[derive(Debug)]
pub(crate) struct CompiledTokenRegex {
    patterns: Vec<regex::Regex>,
    compile_duration: Duration,
}

impl CompiledTokenRegex {
    pub(crate) fn pattern_count(&self) -> usize {
        self.patterns.len()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RegexExpansion {
    pub(crate) patterns: usize,
    pub(crate) tokens_examined: usize,
    pub(crate) expanded_tokens: Vec<String>,
    pub(crate) pattern_match_counts: Vec<usize>,
    pub(crate) posting_telemetry: TokenSearchTelemetry,
    strategy: TokenRegexExpansionStrategy,
    strategy_reason: TokenRegexStrategyReason,
    accounting_scope: TokenRegexAccountingScope,
    timings: RegexExpansionTimings,
}

impl RegexExpansion {
    pub(crate) fn empty(compiled: &CompiledTokenRegex) -> Self {
        Self {
            patterns: compiled.pattern_count(),
            tokens_examined: 0,
            expanded_tokens: Vec::new(),
            pattern_match_counts: vec![0; compiled.pattern_count()],
            posting_telemetry: TokenSearchTelemetry::default(),
            strategy: TokenRegexExpansionStrategy::EmptyScope,
            strategy_reason: TokenRegexStrategyReason::EmptyResolvedScope,
            accounting_scope: TokenRegexAccountingScope::None,
            timings: RegexExpansionTimings {
                compile: compiled.compile_duration,
                ..RegexExpansionTimings::default()
            },
        }
    }

    pub(crate) fn set_plan_duration(&mut self, duration: Duration) {
        self.timings.plan = duration;
    }

    pub(crate) fn set_expansion_total_duration(&mut self, duration: Duration) {
        self.timings.expansion_total = duration;
    }

    pub(crate) fn set_posting_score_duration(&mut self, duration: Duration) {
        self.timings.posting_score = duration;
    }

    pub(crate) fn to_json(&self, count_only: bool) -> Value {
        let mut value = json!({
            "schemaVersion": 2,
            "strategy": self.strategy.as_str(),
            "strategyReason": self.strategy_reason.as_str(),
            "accountingScope": self.accounting_scope.as_str(),
            "patterns": self.patterns,
            "tokensExamined": self.tokens_examined,
            "matchedTokenCount": self.expanded_tokens.len(),
            "postingListsVisited": self.posting_telemetry.posting_lists_visited,
            "postingsChecked": self.posting_telemetry.postings_checked,
            "postingsInScope": self.posting_telemetry.postings_in_scope,
            "timings": {
                "planMs": duration_ms(self.timings.plan),
                "compileMs": duration_ms(self.timings.compile),
                "universeBuildMs": duration_ms(self.timings.universe_build),
                "scanCollectMs": duration_ms(self.timings.scan_collect),
                "sortDedupMs": duration_ms(self.timings.sort_dedup),
                "expansionTotalMs": duration_ms(self.timings.expansion_total),
                "postingScoreMs": duration_ms(self.timings.posting_score),
            },
        });
        if !count_only {
            let preview: Vec<&str> = self.expanded_tokens.iter()
                .take(TOKEN_REGEX_PREVIEW_MAX)
                .map(String::as_str)
                .collect();
            value["matchedTokenPreview"] = json!(preview);
            value["previewTruncated"] = json!(
                self.expanded_tokens.len().saturating_sub(TOKEN_REGEX_PREVIEW_MAX)
            );
        }
        value
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

pub(crate) fn compile_token_regex_patterns(
    raw_terms: &[String],
) -> Result<CompiledTokenRegex, RegexExpansionError> {
    let started = Instant::now();
    let mut patterns = Vec::with_capacity(raw_terms.len());
    for pattern in raw_terms {
        let compiled = regex::Regex::new(&format!("(?i)^{}$", pattern))
            .map_err(|source| RegexExpansionError {
                pattern: pattern.clone(),
                source,
            })?;
        patterns.push(compiled);
    }
    Ok(CompiledTokenRegex {
        patterns,
        compile_duration: started.elapsed(),
    })
}

pub(crate) fn expand_compiled_token_regex<'a, TokenIterator>(
    compiled: &CompiledTokenRegex,
    tokens: TokenIterator,
    dedup: RegexExpansionDedup,
) -> RegexExpansion
where
    TokenIterator: IntoIterator<Item = &'a str> + Clone,
{
    let expansion_started = Instant::now();
    let scan_started = Instant::now();
    let mut expanded_tokens = Vec::new();
    let mut pattern_match_counts = Vec::with_capacity(compiled.pattern_count());
    let mut tokens_examined = 0usize;

    for pattern in &compiled.patterns {
        let mut pattern_matches = 0usize;
        for token in tokens.clone() {
            tokens_examined = tokens_examined.saturating_add(1);
            if pattern.is_match(token) {
                pattern_matches = pattern_matches.saturating_add(1);
                expanded_tokens.push(token.to_string());
            }
        }
        pattern_match_counts.push(pattern_matches);
    }
    let scan_collect_duration = scan_started.elapsed();

    let sort_dedup_duration = if dedup == RegexExpansionDedup::DeduplicateSorted {
        let sort_dedup_started = Instant::now();
        expanded_tokens.sort();
        expanded_tokens.dedup();
        sort_dedup_started.elapsed()
    } else {
        Duration::ZERO
    };

    RegexExpansion {
        patterns: compiled.pattern_count(),
        tokens_examined,
        expanded_tokens,
        pattern_match_counts,
        posting_telemetry: TokenSearchTelemetry::default(),
        strategy: TokenRegexExpansionStrategy::GlobalVocabulary,
        strategy_reason: TokenRegexStrategyReason::GlobalVocabularyBaseline,
        accounting_scope: TokenRegexAccountingScope::GlobalVocabulary,
        timings: RegexExpansionTimings {
            compile: compiled.compile_duration,
            scan_collect: scan_collect_duration,
            sort_dedup: sort_dedup_duration,
            expansion_total: expansion_started.elapsed(),
            ..RegexExpansionTimings::default()
        },
    }
}

pub(crate) fn expand_regex_terms_preserving_duplicates(
    raw_terms: &[String],
    index_keys: &HashMap<String, Vec<Posting>>,
) -> Result<RegexExpansion, RegexExpansionError> {
    let compiled = compile_token_regex_patterns(raw_terms)?;
    Ok(expand_compiled_token_regex(
        &compiled,
        index_keys.keys().map(String::as_str),
        RegexExpansionDedup::PreservePatternDuplicates,
    ))
}

