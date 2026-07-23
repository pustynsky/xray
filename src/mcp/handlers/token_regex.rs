use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::{ContentIndex, Posting};

use super::file_scope::ResolvedFileScope;

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
    ScopedFileTokens,
    GlobalVocabulary,
}

impl TokenRegexExpansionStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::EmptyScope => "emptyScope",
            Self::ScopedFileTokens => "scopedFileTokens",
            Self::GlobalVocabulary => "globalVocabulary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenRegexStrategyReason {
    EmptyResolvedScope,
    ScopeUnfiltered,
    ReverseMapUnavailable,
    ReverseMapRebuildPending,
    ReverseMapInvalid,
    TokenReferenceEstimateTooHigh,
    ScopedCostPreferred,
    GlobalVocabularyBaseline,
    TestForcedGlobal,
    TestForcedScoped,
}

impl TokenRegexStrategyReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::EmptyResolvedScope => "emptyResolvedScope",
            Self::ScopeUnfiltered => "scopeUnfiltered",
            Self::ReverseMapUnavailable => "reverseMapUnavailable",
            Self::ReverseMapRebuildPending => "reverseMapRebuildPending",
            Self::ReverseMapInvalid => "reverseMapInvalid",
            Self::TokenReferenceEstimateTooHigh => "tokenReferenceEstimateTooHigh",
            Self::ScopedCostPreferred => "scopedCostPreferred",
            Self::GlobalVocabularyBaseline => "globalVocabularyBaseline",
            Self::TestForcedGlobal => "testForcedGlobal",
            Self::TestForcedScoped => "testForcedScoped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenRegexAccountingScope {
    None,
    ResolvedFiles,
    GlobalVocabulary,
}

impl TokenRegexAccountingScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ResolvedFiles => "resolvedFiles",
            Self::GlobalVocabulary => "globalVocabulary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopedFileTokensInvalidReason {
    LengthMismatch,
    ScopeIdOutOfRange,
    EmptyLiveSlot,
}

impl ScopedFileTokensInvalidReason {
    fn fallback_reason(self) -> &'static str {
        match self {
            Self::LengthMismatch => "fileTokensLengthMismatch",
            Self::ScopeIdOutOfRange => "fileTokensScopeIdOutOfRange",
            Self::EmptyLiveSlot => "fileTokensEmptyLiveSlot",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopedFileTokensEligibility {
    Ready,
    Unavailable,
    RebuildPending,
    Inconsistent(ScopedFileTokensInvalidReason),
}

impl ScopedFileTokensEligibility {
    fn planner_reason(self) -> TokenRegexStrategyReason {
        match self {
            Self::Ready => TokenRegexStrategyReason::ScopedCostPreferred,
            Self::Unavailable => TokenRegexStrategyReason::ReverseMapUnavailable,
            Self::RebuildPending => TokenRegexStrategyReason::ReverseMapRebuildPending,
            Self::Inconsistent(_) => TokenRegexStrategyReason::ReverseMapInvalid,
        }
    }

    fn fallback_reason(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::Unavailable => Some("fileTokensUnavailable"),
            Self::RebuildPending => Some("fileTokensRebuildPending"),
            Self::Inconsistent(reason) => Some(reason.fallback_reason()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenRegexStrategyOverride {
    Auto,
    ForceGlobal,
    ForceScoped,
}


// Criterion threshold sweep (T=1k/10k/50k, low/medium/hot sharing) showed
// scoped expansion remained neutral or faster through a 50% reference estimate,
// while 10% and 25% caused multi-millisecond global fallbacks for broad regexes.
pub(crate) const DEFAULT_SCOPED_REFERENCE_RATIO_PERCENT: usize = 50;

#[derive(Debug, Clone, Copy)]
struct TokenRegexExpansionDecision {
    strategy: TokenRegexExpansionStrategy,
    reason: TokenRegexStrategyReason,
    scope_files: usize,
    global_unique_tokens: usize,
    scope_token_references: usize,
    fallback_reason: Option<&'static str>,
}

fn plan_token_regex_expansion(
    index: &ContentIndex,
    scope: &ResolvedFileScope,
    strategy_override: TokenRegexStrategyOverride,
    scoped_reference_ratio_percent: usize,
) -> TokenRegexExpansionDecision {
    let global_unique_tokens = index.index.len();
    let scope_files = scope.len();
    if scope.is_empty() {
        return TokenRegexExpansionDecision {
            strategy: TokenRegexExpansionStrategy::EmptyScope,
            reason: TokenRegexStrategyReason::EmptyResolvedScope,
            scope_files,
            global_unique_tokens,
            scope_token_references: 0,
            fallback_reason: None,
        };
    }
    if scope.is_all() {
        return TokenRegexExpansionDecision {
            strategy: TokenRegexExpansionStrategy::GlobalVocabulary,
            reason: TokenRegexStrategyReason::ScopeUnfiltered,
            scope_files,
            global_unique_tokens,
            scope_token_references: 0,
            fallback_reason: None,
        };
    }
    if strategy_override == TokenRegexStrategyOverride::ForceGlobal {
        return TokenRegexExpansionDecision {
            strategy: TokenRegexExpansionStrategy::GlobalVocabulary,
            reason: TokenRegexStrategyReason::TestForcedGlobal,
            scope_files,
            global_unique_tokens,
            scope_token_references: 0,
            fallback_reason: None,
        };
    }

    let eligibility = scoped_file_tokens_eligibility(index, scope);
    if eligibility != ScopedFileTokensEligibility::Ready {
        return TokenRegexExpansionDecision {
            strategy: TokenRegexExpansionStrategy::GlobalVocabulary,
            reason: eligibility.planner_reason(),
            scope_files,
            global_unique_tokens,
            scope_token_references: 0,
            fallback_reason: eligibility.fallback_reason(),
        };
    }

    let scope_token_references = scope.iter_ids().fold(0usize, |total, file_id| {
        total.saturating_add(index.file_tokens[file_id as usize].len())
    });
    if strategy_override != TokenRegexStrategyOverride::ForceScoped {
        let scoped_threshold = global_unique_tokens
            .saturating_mul(scoped_reference_ratio_percent)
            .saturating_add(99)
            / 100;
        if global_unique_tokens > 0 && scope_token_references >= scoped_threshold {
            return TokenRegexExpansionDecision {
                strategy: TokenRegexExpansionStrategy::GlobalVocabulary,
                reason: TokenRegexStrategyReason::TokenReferenceEstimateTooHigh,
                scope_files,
                global_unique_tokens,
                scope_token_references,
                fallback_reason: Some("scopeTokenReferencesTooHigh"),
            };
        }
    }

    TokenRegexExpansionDecision {
        strategy: TokenRegexExpansionStrategy::ScopedFileTokens,
        reason: if strategy_override == TokenRegexStrategyOverride::ForceScoped {
            TokenRegexStrategyReason::TestForcedScoped
        } else {
            TokenRegexStrategyReason::ScopedCostPreferred
        },
        scope_files,
        global_unique_tokens,
        scope_token_references,
        fallback_reason: None,
    }
}

pub(crate) fn scoped_file_tokens_eligibility(
    index: &ContentIndex,
    scope: &ResolvedFileScope,
) -> ScopedFileTokensEligibility {
    if !index.file_tokens_authoritative {
        return ScopedFileTokensEligibility::Unavailable;
    }
    if index.file_tokens.is_empty() {
        return ScopedFileTokensEligibility::RebuildPending;
    }
    if index.file_tokens.len() != index.files.len() {
        return ScopedFileTokensEligibility::Inconsistent(
            ScopedFileTokensInvalidReason::LengthMismatch,
        );
    }

    for file_id in scope.iter_ids() {
        let file_id = file_id as usize;
        let Some(tokens) = index.file_tokens.get(file_id) else {
            return ScopedFileTokensEligibility::Inconsistent(
                ScopedFileTokensInvalidReason::ScopeIdOutOfRange,
            );
        };
        let is_live = index.files.get(file_id).is_some_and(|path| !path.is_empty());
        let token_count = index.file_token_counts.get(file_id).copied().unwrap_or(0);
        if is_live && token_count > 0 && tokens.is_empty() {
            return ScopedFileTokensEligibility::Inconsistent(
                ScopedFileTokensInvalidReason::EmptyLiveSlot,
            );
        }
    }

    ScopedFileTokensEligibility::Ready
}

#[cfg(test)]
pub(crate) fn verify_file_tokens_bidirectional(index: &ContentIndex) -> Result<(), String> {
    if !index.file_tokens_authoritative {
        return Err("file_tokens is not authoritative".to_string());
    }
    if index.file_tokens.len() != index.files.len() {
        return Err(format!(
            "file_tokens length {} does not match file slots {}",
            index.file_tokens.len(),
            index.files.len(),
        ));
    }

    for (file_id, tokens) in index.file_tokens.iter().enumerate() {
        if tokens.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(format!(
                "reverse slot {file_id} is not strictly sorted and deduplicated",
            ));
        }
        for token in tokens {
            let has_forward_posting = index.index.get(token).is_some_and(|postings| {
                postings.iter().any(|posting| posting.file_id as usize == file_id)
            });
            if !has_forward_posting {
                return Err(format!(
                    "reverse token '{token}' in slot {file_id} has no forward posting",
                ));
            }
        }
    }

    for (token, postings) in &index.index {
        for posting in postings {
            let file_id = posting.file_id as usize;
            let Some(tokens) = index.file_tokens.get(file_id) else {
                return Err(format!(
                    "forward token '{token}' references out-of-range file_id {file_id}",
                ));
            };
            if tokens.binary_search(token).is_err() {
                return Err(format!(
                    "forward token '{token}' for file_id {file_id} is missing from reverse slot",
                ));
            }
        }
    }

    Ok(())
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
    scope_files: usize,
    global_unique_tokens: usize,
    scope_token_references: usize,
    scope_unique_tokens: usize,
    global_matched_token_count_known: bool,
    fallback_reason: Option<&'static str>,
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
            scope_files: 0,
            global_unique_tokens: 0,
            scope_token_references: 0,
            scope_unique_tokens: 0,
            global_matched_token_count_known: false,
            fallback_reason: None,
            timings: RegexExpansionTimings {
                compile: compiled.compile_duration,
                ..RegexExpansionTimings::default()
            },
        }
    }

    pub(crate) fn set_posting_score_duration(&mut self, duration: Duration) {
        self.timings.posting_score = duration;
    }

    // The Criterion target path-includes this module, so the production crate
    // does not see the call site even though the benchmark copy uses it.
    #[allow(dead_code)]
    pub(crate) fn phase_durations(
        &self,
    ) -> (Duration, Duration, Duration, Duration, Duration, Duration, Duration) {
        (
            self.timings.plan,
            self.timings.compile,
            self.timings.universe_build,
            self.timings.scan_collect,
            self.timings.sort_dedup,
            self.timings.expansion_total,
            self.timings.posting_score,
        )
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
            "globalUniqueTokens": self.global_unique_tokens,
            "scopeFiles": self.scope_files,
            "scopeTokenReferences": self.scope_token_references,
            "scopeUniqueTokens": self.scope_unique_tokens,
            "globalMatchedTokenCountKnown": self.global_matched_token_count_known,
            "fallbackReason": self.fallback_reason,
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

    let universe_token_count = tokens_examined
        .checked_div(compiled.pattern_count())
        .unwrap_or(0);
    RegexExpansion {
        patterns: compiled.pattern_count(),
        tokens_examined,
        expanded_tokens,
        pattern_match_counts,
        posting_telemetry: TokenSearchTelemetry::default(),
        strategy: TokenRegexExpansionStrategy::GlobalVocabulary,
        strategy_reason: TokenRegexStrategyReason::GlobalVocabularyBaseline,
        accounting_scope: TokenRegexAccountingScope::GlobalVocabulary,
        scope_files: 0,
        global_unique_tokens: universe_token_count,
        scope_token_references: 0,
        scope_unique_tokens: 0,
        global_matched_token_count_known: true,
        fallback_reason: None,
        timings: RegexExpansionTimings {
            compile: compiled.compile_duration,
            scan_collect: scan_collect_duration,
            sort_dedup: sort_dedup_duration,
            expansion_total: expansion_started.elapsed(),
            ..RegexExpansionTimings::default()
        },
    }
}


pub(crate) fn expand_compiled_token_regex_for_scope(
    compiled: &CompiledTokenRegex,
    index: &ContentIndex,
    scope: &ResolvedFileScope,
    strategy_override: TokenRegexStrategyOverride,
) -> RegexExpansion {
    expand_compiled_token_regex_for_scope_with_threshold(
        compiled,
        index,
        scope,
        strategy_override,
        DEFAULT_SCOPED_REFERENCE_RATIO_PERCENT,
    )
}

pub(crate) fn expand_compiled_token_regex_for_scope_with_threshold(
    compiled: &CompiledTokenRegex,
    index: &ContentIndex,
    scope: &ResolvedFileScope,
    strategy_override: TokenRegexStrategyOverride,
    scoped_reference_ratio_percent: usize,
) -> RegexExpansion {
    let expansion_started = Instant::now();
    let plan_started = Instant::now();
    let decision = plan_token_regex_expansion(
        index,
        scope,
        strategy_override,
        scoped_reference_ratio_percent,
    );
    let plan_duration = plan_started.elapsed();

    let (mut expansion, scope_unique_tokens, universe_build_duration) = match decision.strategy {
        TokenRegexExpansionStrategy::EmptyScope => (RegexExpansion::empty(compiled), 0, Duration::ZERO),
        TokenRegexExpansionStrategy::GlobalVocabulary => (
            expand_compiled_token_regex(
                compiled,
                index.index.keys().map(String::as_str),
                RegexExpansionDedup::DeduplicateSorted,
            ),
            0,
            Duration::ZERO,
        ),
        TokenRegexExpansionStrategy::ScopedFileTokens if scope.len() == 1 => {
            let universe_started = Instant::now();
            let file_id = scope.iter_ids().next().expect("single-file scope has no file id") as usize;
            let tokens = &index.file_tokens[file_id];
            let universe_build_duration = universe_started.elapsed();
            (
                expand_compiled_token_regex(
                    compiled,
                    tokens.iter().map(String::as_str),
                    RegexExpansionDedup::DeduplicateSorted,
                ),
                tokens.len(),
                universe_build_duration,
            )
        }
        TokenRegexExpansionStrategy::ScopedFileTokens => {
            let universe_started = Instant::now();
            let mut unique_tokens = HashSet::new();
            for file_id in scope.iter_ids() {
                unique_tokens.extend(index.file_tokens[file_id as usize].iter().map(String::as_str));
            }
            let mut tokens: Vec<&str> = unique_tokens.into_iter().collect();
            tokens.sort_unstable();
            let universe_build_duration = universe_started.elapsed();
            let scope_unique_tokens = tokens.len();
            (
                expand_compiled_token_regex(
                    compiled,
                    tokens.iter().copied(),
                    RegexExpansionDedup::DeduplicateSorted,
                ),
                scope_unique_tokens,
                universe_build_duration,
            )
        }
    };

    expansion.strategy = decision.strategy;
    expansion.strategy_reason = decision.reason;
    expansion.accounting_scope = match decision.strategy {
        TokenRegexExpansionStrategy::EmptyScope => TokenRegexAccountingScope::None,
        TokenRegexExpansionStrategy::ScopedFileTokens => TokenRegexAccountingScope::ResolvedFiles,
        TokenRegexExpansionStrategy::GlobalVocabulary => TokenRegexAccountingScope::GlobalVocabulary,
    };
    expansion.scope_files = decision.scope_files;
    expansion.global_unique_tokens = decision.global_unique_tokens;
    expansion.scope_token_references = decision.scope_token_references;
    expansion.scope_unique_tokens = scope_unique_tokens;
    // A scoped expansion intentionally does not know the global match count.
    // Computing it would require a second regex scan over the global vocabulary
    // solely for response accounting, defeating the scoped strategy.
    expansion.global_matched_token_count_known =
        decision.strategy == TokenRegexExpansionStrategy::GlobalVocabulary;
    expansion.fallback_reason = decision.fallback_reason;
    expansion.timings.plan = plan_duration;
    expansion.timings.universe_build = universe_build_duration;
    expansion.timings.expansion_total = expansion_started.elapsed();
    expansion
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

