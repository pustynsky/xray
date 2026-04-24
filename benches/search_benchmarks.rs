//! Criterion benchmarks for search engine core operations.
//!
//! # How to run
//!
//! ```text
//! cargo bench --bench search_benchmarks
//! ```
//!
//! HTML reports are written to `target/criterion/`. To compare against a
//! saved baseline use `cargo bench -- --save-baseline <name>` and `critcmp`.
//!
//! # Important caveats
//!
//! These benchmarks measure the core operations in **isolation against
//! synthetic data** so results are reproducible across machines. Several
//! known fidelity gaps are tracked in
//! `docs/user-stories/todo_2026-04-22_benches-review-findings.md`:
//!
//! * Some scoring/intersection helpers are inlined copies of production
//!   logic rather than calls into `code_xray::*` (BENCH-001..003).
//! * The synthetic corpus is uniform/Zipf-free and does not reflect real
//!   token-frequency distributions (BENCH-004).
//! * MCP handler hot paths (`xray_grep`, `xray_definitions`, `xray_callers`,
//!   `xray_edit`, `xray_git_*`) are not yet covered (BENCH-005).
//! * `bench_serialization` measures bincode without LZ4-frame compression
//!   used in production (BENCH-007).
//!
//! Treat absolute numbers as ballpark figures — the regression-tracking
//! value of these benches is in *deltas* across commits, not in absolute
//! latency claims about production behaviour.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::collections::HashMap;

// Import from the code-xray crate
use code_xray::{generate_trigrams, tokenize, ContentIndex, Posting, TrigramIndex};

// ─── Shared parameter sets (BENCH-018) ───────────────────────────────

/// Standard file-count sweep used by most benches. Keep in sync with the
/// regression-tracking baselines under `target/criterion/`.
const BENCH_SIZES: &[usize] = &[1_000, 10_000, 50_000];

/// Reduced sweep for benches whose per-iteration cost is dominated by
/// allocation / serialization, where 50k files would push wall-clock past
/// the criterion default measurement window.
const BENCH_SIZES_SMALL: &[usize] = &[100, 1_000, 5_000];

// ─── Helpers ─────────────────────────────────────────────────────────

/// Build a synthetic ContentIndex with N files, each containing a set of tokens.
fn build_synthetic_index(num_files: usize, tokens_per_file: usize) -> ContentIndex {
    let mut files = Vec::with_capacity(num_files);
    let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
    let mut file_token_counts = Vec::with_capacity(num_files);
    let mut total_tokens: u64 = 0;

    for file_id in 0..num_files {
        files.push(format!("src/file_{}.cs", file_id));
        // BENCH-011: accumulate in u64 so a future bump to `tokens_per_file`
        // beyond ~4.29e9 fails loudly on the `try_from` below instead of
        // silently wrapping a `u32` counter.
        let mut count: u64 = 0;

        for t in 0..tokens_per_file {
            let token = format!("token_{}", t % 500); // 500 unique tokens
            total_tokens += 1;
            count += 1;
            let line = (t + 1) as u32;

            index
                .entry(token)
                .or_default()
                .push(Posting {
                    file_id: file_id as u32,
                    lines: vec![line],
                });
        }

        // Add some common tokens to every file
        for common in &["class", "public", "void", "return", "using", "namespace"] {
            let token = common.to_string();
            total_tokens += 1;
            count += 1;
            index
                .entry(token)
                .or_default()
                .push(Posting {
                    file_id: file_id as u32,
                    lines: vec![1],
                });
        }

        // Add a rare token to only 1% of files
        if file_id % 100 == 0 {
            let token = "rarehttpclient".to_string();
            total_tokens += 1;
            count += 1;
            index
                .entry(token)
                .or_default()
                .push(Posting {
                    file_id: file_id as u32,
                    lines: vec![5, 12, 30],
                });
        }

        let count_u32 = u32::try_from(count).expect(
            "per-file token count overflows u32; lower tokens_per_file or widen ContentIndex::file_token_counts",
        );
        file_token_counts.push(count_u32);
    }

    ContentIndex {
        root: ".".to_string(),
        created_at: 0,
        max_age_secs: 86400,
        files,
        index,
        total_tokens,
        extensions: vec!["cs".to_string()],
        file_token_counts,
        ..Default::default()
    }
}

// ─── Tokenizer Benchmarks ────────────────────────────────────────────

fn bench_tokenize(c: &mut Criterion) {
    let mut group = c.benchmark_group("tokenize");

    let short_line = "private readonly HttpClient _client;";
    let medium_line = "public async Task<IEnumerable<SearchResult>> ExecuteQueryAsync(string query, CancellationToken cancellationToken = default)";
    let long_line = "var result = await _serviceProvider.GetRequiredService<IQueryHandler>().ExecuteAsync(new QueryRequest { UserId = userId, Query = query, MaxResults = maxResults, IncludeMetadata = true, Timeout = TimeSpan.FromSeconds(30) }, cancellationToken).ConfigureAwait(false);";

    group.bench_function("short_line", |b| {
        b.iter(|| tokenize(black_box(short_line), 2))
    });

    group.bench_function("medium_line", |b| {
        b.iter(|| tokenize(black_box(medium_line), 2))
    });

    group.bench_function("long_line", |b| {
        b.iter(|| tokenize(black_box(long_line), 2))
    });

    // Tokenize a block of code (multi-line)
    let code_block = r#"
using System;
using System.Collections.Generic;
using System.Threading.Tasks;

namespace MyApp.Services
{
    public class UserService : IUserService
    {
        private readonly ILogger<UserService> _logger;
        private readonly HttpClient _httpClient;
        private readonly IMemoryCache _cache;

        public UserService(
            ILogger<UserService> logger,
            HttpClient httpClient,
            IMemoryCache cache)
        {
            _logger = logger;
            _httpClient = httpClient;
            _cache = cache;
        }

        public async Task<QueryResult> ExecuteAsync(string query)
        {
            _logger.LogInformation("Executing query: {Query}", query);
            var result = await _httpClient.GetAsync($"/api/search?q={query}");
            return await result.Content.ReadAsAsync<QueryResult>();
        }
    }
}
"#;

    group.bench_function("code_block_30_lines", |b| {
        b.iter(|| {
            let mut tokens = Vec::new();
            for line in black_box(code_block).lines() {
                tokens.extend(tokenize(line, 2));
            }
            tokens
        })
    });

    group.finish();
}

// ─── Index Lookup Benchmarks ─────────────────────────────────────────

fn bench_index_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_lookup");

    // Test with different index sizes
    for &num_files in BENCH_SIZES {
        let index = build_synthetic_index(num_files, 200);

        group.bench_with_input(
            BenchmarkId::new("single_token", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    black_box(index.index.get("token_42"));
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("common_token", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    black_box(index.index.get("class"));
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("rare_token", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    black_box(index.index.get("rarehttpclient"));
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("missing_token", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    black_box(index.index.get("nonexistent_token_xyz"));
                })
            },
        );
    }

    group.finish();
}

// ─── TF-IDF Scoring Benchmarks ───────────────────────────────────────

fn bench_tfidf_scoring(c: &mut Criterion) {
    let mut group = c.benchmark_group("tfidf_scoring");

    for &num_files in BENCH_SIZES {
        let index = build_synthetic_index(num_files, 200);
        let total_docs = index.files.len() as f64;

        group.bench_with_input(
            BenchmarkId::new("score_single_term", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    let token = "token_42";
                    if let Some(postings) = index.index.get(token) {
                        let doc_freq = postings.len() as f64;
                        let idf = (total_docs / doc_freq).ln();
                        let mut scores: Vec<(u32, f64)> = Vec::new();
                        for posting in postings {
                            let file_total =
                                index.file_token_counts[posting.file_id as usize] as f64;
                            let tf = posting.lines.len() as f64 / file_total;
                            scores.push((posting.file_id, tf * idf));
                        }
                        scores.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        black_box(scores);
                    }
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("score_multi_term_3", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    let terms = ["token_1", "token_42", "token_100"];
                    let mut file_scores: HashMap<u32, f64> = HashMap::new();

                    for token in &terms {
                        if let Some(postings) = index.index.get(*token) {
                            let doc_freq = postings.len() as f64;
                            let idf = (total_docs / doc_freq).ln();
                            for posting in postings {
                                let file_total =
                                    index.file_token_counts[posting.file_id as usize] as f64;
                                let tf = posting.lines.len() as f64 / file_total;
                                *file_scores.entry(posting.file_id).or_default() += tf * idf;
                            }
                        }
                    }

                    let mut results: Vec<_> = file_scores.into_iter().collect();
                    results.sort_by(|a, b| {
                        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    black_box(results);
                })
            },
        );
    }

    group.finish();
}

// ─── Regex Token Scan Benchmarks ─────────────────────────────────────

fn bench_regex_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("regex_token_scan");

    let re_prefix = regex::Regex::new("(?i)^token_4.*$").unwrap();
    let re_exact = regex::Regex::new("(?i)^class$").unwrap();

    for &num_files in BENCH_SIZES {
        let index = build_synthetic_index(num_files, 200);

        group.bench_with_input(
            BenchmarkId::new("scan_all_keys", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    let matches: Vec<&String> =
                        index.index.keys().filter(|k| re_prefix.is_match(k)).collect();
                    black_box(matches);
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("scan_prefix_pattern", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    let matches: Vec<&String> =
                        index.index.keys().filter(|k| re_exact.is_match(k)).collect();
                    black_box(matches);
                })
            },
        );
    }

    group.finish();
}

// ─── Index Build Benchmarks ──────────────────────────────────────────

fn bench_index_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_build");
    group.sample_size(10); // Slower benchmarks need fewer samples

    for &num_files in BENCH_SIZES_SMALL {
        group.bench_with_input(
            BenchmarkId::new("build_synthetic", num_files),
            &num_files,
            |b, &num_files| {
                b.iter(|| {
                    black_box(build_synthetic_index(num_files, 200));
                })
            },
        );
    }

    group.finish();
}

// ─── Serialization Benchmarks ────────────────────────────────────────

fn bench_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization");
    group.sample_size(10);

    let index = build_synthetic_index(5_000, 200);

    group.bench_function("serialize_5k_files", |b| {
        b.iter(|| {
            let encoded = bincode::serialize(black_box(&index)).unwrap();
            black_box(encoded.len());
        })
    });

    let encoded = bincode::serialize(&index).unwrap();
    let encoded_len = encoded.len();

    group.bench_function("deserialize_5k_files", |b| {
        b.iter(|| {
            let decoded: ContentIndex = bincode::deserialize(black_box(&encoded)).unwrap();
            black_box(decoded.files.len());
        })
    });

    group.bench_function(
        format!("serialize_size_bytes_{}", encoded_len),
        |b| {
            b.iter(|| black_box(encoded_len))
        },
    );

    group.finish();
}

// ─── Trigram / Substring Benchmarks ─────────────────────────────────

/// Build a TrigramIndex from an inverted index (mirrors build_trigram_index in index.rs)
fn build_trigram_for_bench(inverted: &HashMap<String, Vec<Posting>>) -> TrigramIndex {
    let mut tokens: Vec<String> = inverted.keys().cloned().collect();
    tokens.sort();

    let mut trigram_map: HashMap<String, Vec<u32>> = HashMap::new();

    for (idx, token) in tokens.iter().enumerate() {
        let trigrams = generate_trigrams(token);
        for trigram in trigrams {
            trigram_map.entry(trigram).or_default().push(idx as u32);
        }
    }

    for list in trigram_map.values_mut() {
        list.sort();
        list.dedup();
    }

    TrigramIndex { tokens, trigram_map }
}

/// Sorted intersection of two u32 slices (mirrors sorted_intersect in handlers.rs)
fn sorted_intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => { result.push(a[i]); i += 1; j += 1; }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    result
}

fn bench_trigram_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("trigram_build");
    group.sample_size(10);

    for &num_files in BENCH_SIZES {
        let index = build_synthetic_index(num_files, 200);

        group.bench_with_input(
            BenchmarkId::new("build_trigram_index", num_files),
            &index,
            |b, index| {
                b.iter(|| {
                    black_box(build_trigram_for_bench(&index.index));
                })
            },
        );
    }

    group.finish();
}

fn bench_substring_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("substring_search");

    for &num_files in BENCH_SIZES {
        let index = build_synthetic_index(num_files, 200);
        let trigram = build_trigram_for_bench(&index.index);

        // Long query (10+ chars) — uses trigram intersection
        group.bench_with_input(
            BenchmarkId::new("long_query_12chars", num_files),
            &(&index, &trigram),
            |b, &(index, trigram)| {
                b.iter(|| {
                    let query = "rarehttpclie"; // 12 chars, partial match
                    let query_lower = query.to_lowercase();
                    let query_trigrams = generate_trigrams(&query_lower);

                    let mut candidates: Option<Vec<u32>> = None;
                    for tri in &query_trigrams {
                        if let Some(list) = trigram.trigram_map.get(tri) {
                            candidates = Some(match candidates {
                                None => list.clone(),
                                Some(prev) => sorted_intersect(&prev, list),
                            });
                        }
                    }

                    let verified: Vec<&str> = candidates.unwrap_or_default().iter()
                        .filter_map(|&idx| trigram.tokens.get(idx as usize))
                        .filter(|t| t.contains(&query_lower))
                        .map(|t| t.as_str())
                        .collect();

                    // Look up in main index
                    for token in &verified {
                        black_box(index.index.get(*token));
                    }
                    black_box(verified.len());
                })
            },
        );

        // Short query (2 chars) — linear scan fallback
        group.bench_with_input(
            BenchmarkId::new("short_query_2chars", num_files),
            &(&index, &trigram),
            |b, &(_index, trigram)| {
                b.iter(|| {
                    let query = "cl"; // 2 chars — falls back to linear scan
                    let matches: Vec<&str> = trigram.tokens.iter()
                        .filter(|t| t.contains(query))
                        .map(|t| t.as_str())
                        .collect();
                    black_box(matches.len());
                })
            },
        );
    }

    group.finish();
}

fn bench_substring_vs_regex(c: &mut Criterion) {
    let mut group = c.benchmark_group("substring_vs_regex");

    let index = build_synthetic_index(10_000, 200);
    let trigram = build_trigram_for_bench(&index.index);

    // Substring search via trigram
    group.bench_function("trigram_substring", |b| {
        b.iter(|| {
            let query = "rarehttpclie";
            let query_lower = query.to_lowercase();
            let query_trigrams = generate_trigrams(&query_lower);

            let mut candidates: Option<Vec<u32>> = None;
            for tri in &query_trigrams {
                if let Some(list) = trigram.trigram_map.get(tri) {
                    candidates = Some(match candidates {
                        None => list.clone(),
                        Some(prev) => sorted_intersect(&prev, list),
                    });
                }
            }

            let verified: Vec<&str> = candidates.unwrap_or_default().iter()
                .filter_map(|&idx| trigram.tokens.get(idx as usize))
                .filter(|t| t.contains(&query_lower))
                .map(|t| t.as_str())
                .collect();
            black_box(verified.len());
        })
    });

    // Equivalent regex scan of all keys
    group.bench_function("regex_scan_all_keys", |b| {
        let re = regex::Regex::new("(?i).*rarehttpclie.*").unwrap();
        b.iter(|| {
            let matches: Vec<&String> = index.index.keys()
                .filter(|k| re.is_match(k))
                .collect();
            black_box(matches.len());
        })
    });

    // Linear contains() scan of all keys
    group.bench_function("linear_contains_scan", |b| {
        b.iter(|| {
            let query = "rarehttpclie";
            let matches: Vec<&String> = index.index.keys()
                .filter(|k| k.contains(query))
                .collect();
            black_box(matches.len());
        })
    });

    group.finish();
}

// ─── PERF-AUDIT-2026-04-24 micro-benches ─────────────────────────────
//
// These benches isolate hot paths targeted by `docs/user-stories/
// todo_approved_2026-04-24_perf-audit-followups.md`. Each PERF-* PR uses
// `--save-baseline pre-perf-audit` here as its reference point.
//
// Scope (PERF-00 minimum, see story v2 / 2026-04-24 cross-LLM review):
//   * PERF-05: `bench_generate_trigrams` — ASCII vs Unicode token tax
//   * PERF-01: `bench_regex_compile` — per-request `Regex::new` cost vs
//     pre-compiled lookup (motivates the LRU cache)
//   * PERF-04: `bench_top_authors_aggregation` — per-commit `format!` key
//     vs `(String, String)` tuple key
//   * PERF-07: `bench_resolve_parent_substring` — repeated per-node
//     content_index lookups + substring scan (motivates subtree memo)
//
// Out of scope (measured separately by `scripts/bench-git-perf.ps1`):
//   PERF-02 (branch detect), PERF-03 (commit diff), PERF-09 (blame).
//   Spawning `git` from criterion produces high-variance numbers and
//   pulls in repo state setup we don't want to bake into the harness.
//
// Out of scope (would require exposing private MCP handlers behind a
// `bench-internals` feature flag; deferred to a follow-up PR):
//   end-to-end `xray_grep` / `xray_callers` / `xray_fast` handler latency.

fn bench_generate_trigrams(c: &mut Criterion) {
    let mut group = c.benchmark_group("generate_trigrams");

    // ASCII tokens of varying length — the dominant case in real codebases
    // (identifiers like "HttpClient", "ExecuteQueryAsync"). PERF-05's ASCII
    // fast-path skips the `Vec<char>` collect, so improvement here should be
    // visible without touching the Unicode branch.
    let ascii_short = "User"; // 4 chars → 2 trigrams
    let ascii_medium = "HttpClientFactory"; // 17 chars
    let ascii_long = "VeryLongIdentifierNameForTestingTrigrams"; // 40 chars

    group.bench_function("ascii_short_4ch", |b| {
        b.iter(|| black_box(generate_trigrams(black_box(ascii_short))))
    });
    group.bench_function("ascii_medium_17ch", |b| {
        b.iter(|| black_box(generate_trigrams(black_box(ascii_medium))))
    });
    group.bench_function("ascii_long_40ch", |b| {
        b.iter(|| black_box(generate_trigrams(black_box(ascii_long))))
    });

    // Non-ASCII regression guard: PERF-05 must not regress these.
    // Cyrillic identifier (4 chars, 8 bytes UTF-8) and CJK (4 chars, 12 bytes)
    // — both require the `Vec<char>` slow path.
    let cyrillic = "ПользовательСервис"; // 18 chars
    let cjk = "用户服务管理器"; // 7 chars

    group.bench_function("cyrillic_18ch", |b| {
        b.iter(|| black_box(generate_trigrams(black_box(cyrillic))))
    });
    group.bench_function("cjk_7ch", |b| {
        b.iter(|| black_box(generate_trigrams(black_box(cjk))))
    });

    // Whole-vocab build: amortised cost across a synthetic 18k-token vocab
    // (matches the workspace token count cited in xray's MCP responses).
    // PERF-05's win is most visible on this curve.
    let vocab: Vec<String> = (0..18_000)
        .map(|i| format!("identifier_{}_with_some_suffix", i))
        .collect();
    group.bench_function("vocab_18k_tokens", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for t in &vocab {
                total += generate_trigrams(t).len();
            }
            black_box(total);
        })
    });

    group.finish();
}

fn bench_regex_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("regex_compile");

    // The three regex sites PERF-01 caches:
    //   TV: token-vocab anchored, case-insensitive   `(?i)^...$`
    //   PH: phrase substring                         `(?i)foo.*bar`
    //   LR: line regex with multi-line               builder + multi_line(true)
    // We bench compile + match on a small input so the dominant cost is
    // compilation (the cache replaces only this part).
    let tv_pat = "(?i)^user.*service$";
    let ph_pat = "(?i)Http.*Client";
    let lr_pat = "^[ \\t]*pub fn ";
    let sample = "pub fn handle_user_service_request() -> Result<(), Error> {";

    group.bench_function("compile_TV_anchored_ci", |b| {
        b.iter(|| {
            let re = regex::Regex::new(black_box(tv_pat)).unwrap();
            black_box(re.is_match(black_box(sample)));
        })
    });
    group.bench_function("compile_PH_phrase_ci", |b| {
        b.iter(|| {
            let re = regex::Regex::new(black_box(ph_pat)).unwrap();
            black_box(re.is_match(black_box(sample)));
        })
    });
    group.bench_function("compile_LR_multiline", |b| {
        b.iter(|| {
            let re = regex::RegexBuilder::new(black_box(lr_pat))
                .multi_line(true)
                .build()
                .unwrap();
            black_box(re.is_match(black_box(sample)));
        })
    });

    // Reference point: cached path = compile once, match N times.
    // Ratio of (compile_TV / cached_match) gives the per-request overhead
    // PERF-01 will eliminate on cache hits.
    let cached_re = regex::Regex::new(tv_pat).unwrap();
    group.bench_function("cached_TV_match_only", |b| {
        b.iter(|| black_box(cached_re.is_match(black_box(sample))))
    });

    group.finish();
}

fn bench_top_authors_aggregation(c: &mut Criterion) {
    // Mirrors the hot loop in `git::top_authors`:
    //   `format!("{} <{}>", name, email)` per commit → HashMap key.
    // PERF-04 swaps the key to `(String, String)` so the format! happens
    // only N_AUTHORS times instead of N_COMMITS.
    let mut group = c.benchmark_group("top_authors_aggregation");

    // Synthetic distribution: 50 authors, ~50k commits (matches the
    // `--max-count=50000` cap in production).
    let commits: Vec<(String, String)> = (0..50_000)
        .map(|i| {
            let a = i % 50;
            (format!("Author {}", a), format!("a{}@example.com", a))
        })
        .collect();

    group.bench_function("string_key_format_50k", |b| {
        b.iter(|| {
            let mut m: HashMap<String, usize> = HashMap::new();
            for (n, e) in &commits {
                let k = format!("{} <{}>", n, e);
                *m.entry(k).or_default() += 1;
            }
            black_box(m.len());
        })
    });

    group.bench_function("tuple_key_50k", |b| {
        b.iter(|| {
            let mut m: HashMap<(String, String), usize> = HashMap::new();
            for (n, e) in &commits {
                *m.entry((n.clone(), e.clone())).or_default() += 1;
            }
            black_box(m.len());
        })
    });

    group.finish();
}

fn bench_resolve_parent_substring(c: &mut Criterion) {
    // Mirrors the hot loop in callers' `resolve_parent_file_ids` +
    // `collect_substring_file_ids`: for each tree node we do 4-6 HashMap
    // lookups in `content_index.index` plus a substring scan of all
    // matching tokens. PERF-07 memoises this per (parent_class) within a
    // single tree-build.
    let mut group = c.benchmark_group("callers_resolve_substring");

    // Build a synthetic content index where ~5% of files contain a token
    // matching "storagemanager" as a substring (e.g. m_storageManager).
    let index = build_synthetic_index(10_000, 200);
    let mut idx2 = index;
    // Inject substring-matching tokens into the index so the bench actually
    // exercises the substring scan path. Done outside `b.iter` so we measure
    // lookup cost, not setup.
    for (i, fid) in (0..500).enumerate() {
        let tok = format!("m_storagemanager_{}", i);
        idx2.index
            .entry(tok)
            .or_default()
            .push(Posting { file_id: fid as u32, lines: vec![1] });
    }

    let parent_classes = ["StorageManager", "QueryHandler", "UserService"];
    let lowered: Vec<String> = parent_classes
        .iter()
        .map(|s| s.to_lowercase())
        .collect();

    group.bench_function("naive_per_node_3classes_x100", |b| {
        b.iter(|| {
            // Simulate 100 tree nodes resolving 3 distinct parent classes.
            // No memo: every node pays the full 3× lookup + substring scan.
            let mut total_files = 0usize;
            for _node in 0..100 {
                for cls in &lowered {
                    if let Some(p) = idx2.index.get(cls) {
                        total_files += p.len();
                    }
                    let iface = format!("i{}", cls);
                    if let Some(p) = idx2.index.get(&iface) {
                        total_files += p.len();
                    }
                    // Substring scan: linear over keys (matches naive impl).
                    for k in idx2.index.keys() {
                        if k.contains(cls.as_str()) {
                            total_files += 1;
                        }
                    }
                }
            }
            black_box(total_files);
        })
    });

    group.bench_function("memoised_3classes_x100", |b| {
        b.iter(|| {
            // PERF-07 target: resolve each parent class once, reuse for all
            // 100 nodes. Lookup cost amortises 100× → ~3 resolutions total.
            let mut cache: HashMap<&str, usize> = HashMap::new();
            for cls in &lowered {
                let entry = cache.entry(cls.as_str()).or_insert_with(|| {
                    let mut count = 0usize;
                    if let Some(p) = idx2.index.get(cls.as_str()) {
                        count += p.len();
                    }
                    let iface = format!("i{}", cls);
                    if let Some(p) = idx2.index.get(&iface) {
                        count += p.len();
                    }
                    for k in idx2.index.keys() {
                        if k.contains(cls.as_str()) {
                            count += 1;
                        }
                    }
                    count
                });
                let _ = *entry;
            }
            let mut total_files = 0usize;
            for _node in 0..100 {
                for cls in &lowered {
                    total_files += cache.get(cls.as_str()).copied().unwrap_or(0);
                }
            }
            black_box(total_files);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_tokenize,
    bench_index_lookup,
    bench_tfidf_scoring,
    bench_regex_scan,
    bench_index_build,
    bench_serialization,
    bench_trigram_build,
    bench_substring_search,
    bench_substring_vs_regex,
    // PERF-AUDIT-2026-04-24 ───────────────────────────────────────────
    bench_generate_trigrams,
    bench_regex_compile,
    bench_top_authors_aggregation,
    bench_resolve_parent_substring,
);
criterion_main!(benches);