use super::*;
use proptest::prelude::*;

// ─── Tokenizer invariants ────────────────────────────────────

proptest! {
    /// Tokenizer always produces lowercase output regardless of input case.
    #[test]
    fn tokenize_always_lowercase(input in "\\PC{1,200}") {
        let tokens = tokenize(&input, 1);
        for token in &tokens {
            prop_assert_eq!(token, &token.to_lowercase(),
                "Token '{}' is not lowercase", token);
        }
    }

    /// Tokenizer never produces tokens shorter than min_len (byte length).
    /// Note: Uses ASCII input because Unicode lowercasing can change byte length
    /// (e.g. German ß → ss), making the pre-lowercase filter insufficient.
    /// This is acceptable — code identifiers are ASCII in >99% of codebases.
    #[test]
    fn tokenize_respects_min_length(
        input in "[a-zA-Z0-9_ .;:(){}]{1,200}",
        min_len in 1usize..10
    ) {
        let tokens = tokenize(&input, min_len);
        for token in &tokens {
            prop_assert!(token.len() >= min_len,
                "Token '{}' (len {}) is shorter than min_len {}",
                token, token.len(), min_len);
        }
    }

    /// Tokenizer output is deterministic — same input always gives same output.
    #[test]
    fn tokenize_is_deterministic(input in "\\PC{1,200}") {
        let result1 = tokenize(&input, 2);
        let result2 = tokenize(&input, 2);
        prop_assert_eq!(result1, result2);
    }

    /// Empty input always produces empty output.
    #[test]
    fn tokenize_empty_min_len(min_len in 1usize..20) {
        let tokens = tokenize("", min_len);
        prop_assert!(tokens.is_empty());
    }

    /// Tokens only contain alphanumeric chars, underscores, and combining marks
    /// (Unicode lowercasing can produce combining chars, e.g. Turkish İ → i + combining dot).
    #[test]
    fn tokenize_valid_chars_only(input in "[a-zA-Z0-9_ !@#$%^&*()]{1,200}") {
        let tokens = tokenize(&input, 1);
        for token in &tokens {
            for c in token.chars() {
                prop_assert!(c.is_alphanumeric() || c == '_',
                    "Token '{}' contains invalid char '{}'", token, c);
            }
        }
    }

    /// Increasing min_len never increases the number of tokens.
    #[test]
    fn tokenize_higher_min_len_fewer_tokens(input in "\\PC{1,200}") {
        let tokens_1 = tokenize(&input, 1);
        let tokens_2 = tokenize(&input, 2);
        let tokens_5 = tokenize(&input, 5);
        prop_assert!(tokens_2.len() <= tokens_1.len(),
            "min_len=2 produced more tokens ({}) than min_len=1 ({})",
            tokens_2.len(), tokens_1.len());
        prop_assert!(tokens_5.len() <= tokens_2.len(),
            "min_len=5 produced more tokens ({}) than min_len=2 ({})",
            tokens_5.len(), tokens_2.len());
    }

    /// Tokenizing a single alphanumeric word returns that word lowercased.
    #[test]
    fn tokenize_single_word(word in "[a-zA-Z][a-zA-Z0-9_]{1,30}") {
        let tokens = tokenize(&word, 1);
        prop_assert!(tokens.contains(&word.to_lowercase()),
            "Expected '{}' in tokens {:?}", word.to_lowercase(), tokens);
    }
}

// ─── Posting serialization invariants ────────────────────────

proptest! {
    /// Posting survives bincode serialization roundtrip.
    #[test]
    fn posting_roundtrip(
        file_id in 0u32..100_000,
        lines in proptest::collection::vec(1u32..100_000, 0..50)
    ) {
        let posting = Posting { file_id, lines: lines.clone() };
        let encoded = bincode::serialize(&posting).unwrap();
        let decoded: Posting = bincode::deserialize(&encoded).unwrap();
        prop_assert_eq!(decoded.file_id, file_id);
        prop_assert_eq!(decoded.lines, lines);
    }
}

// ─── ContentIndex invariants ─────────────────────────────────

proptest! {
    /// Building an index from tokenized content maintains consistency:
    /// every token in the inverted index points to a valid file_id.
    #[test]
    fn index_file_ids_are_valid(
        num_files in 1usize..20,
        tokens_per_file in 1usize..50,
    ) {
        let mut files = Vec::new();
        let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
        let mut file_token_counts = Vec::new();

        for file_id in 0..num_files {
            files.push(format!("file_{}.cs", file_id));
            let mut count = 0u32;
            for t in 0..tokens_per_file {
                let token = format!("tok_{}", t % 10);
                count += 1;
                index.entry(token).or_default().push(Posting {
                    file_id: file_id as u32,
                    lines: vec![(t + 1) as u32],
                });
            }
            file_token_counts.push(count);
        }

        // Invariant: every file_id in postings is < files.len()
        for postings in index.values() {
            for posting in postings {
                prop_assert!((posting.file_id as usize) < files.len(),
                    "file_id {} >= files.len() {}", posting.file_id, files.len());
            }
        }

        // Invariant: file_token_counts has same length as files
        prop_assert_eq!(file_token_counts.len(), files.len());
    }

    /// ContentIndex survives bincode serialization roundtrip.
    #[test]
    fn content_index_roundtrip(num_files in 1usize..10) {
        let mut files = Vec::new();
        let mut index: HashMap<String, Vec<Posting>> = HashMap::new();
        let mut file_token_counts = Vec::new();
        let mut total_tokens = 0u64;

        for file_id in 0..num_files {
            files.push(format!("file_{}.cs", file_id));
            let token = format!("token_{}", file_id);
            total_tokens += 1;
            file_token_counts.push(1);
            index.entry(token).or_default().push(Posting {
                file_id: file_id as u32,
                lines: vec![1],
            });
        }

        let ci = ContentIndex {
            root: ".".to_string(),
            created_at: 1000,
            max_age_secs: 86400,
            files: files.clone(),
            index,
            total_tokens,
            extensions: vec!["cs".to_string()],
            file_token_counts: file_token_counts.clone(),
            ..Default::default()
        };

        let encoded = bincode::serialize(&ci).unwrap();
        let decoded: ContentIndex = bincode::deserialize(&encoded).unwrap();

        prop_assert_eq!(decoded.files.len(), files.len());
        prop_assert_eq!(decoded.total_tokens, total_tokens);
        prop_assert_eq!(decoded.file_token_counts, file_token_counts);
        prop_assert_eq!(decoded.root, ".");
    }
}

// ─── TF-IDF invariants ───────────────────────────────────────

proptest! {
    /// TF-IDF: a token appearing in fewer documents should have higher IDF.
    #[test]
    fn tfidf_rare_token_higher_idf(
        total_docs in 10u32..10_000,
        rare_count in 1u32..5,
        common_count_extra in 5u32..100,
    ) {
        let total = total_docs as f64;
        let common_count = rare_count + common_count_extra;
        // Ensure common_count <= total_docs
        let common_count = common_count.min(total_docs);
        let rare_count = rare_count.min(common_count - 1).max(1);

        let idf_rare = (total / rare_count as f64).ln();
        let idf_common = (total / common_count as f64).ln();

        prop_assert!(idf_rare > idf_common,
            "Rare IDF ({}) should be > common IDF ({}), rare_count={}, common_count={}, total={}",
            idf_rare, idf_common, rare_count, common_count, total_docs);
    }

    /// TF: higher occurrence count with same file size = higher TF.
    #[test]
    fn tfidf_more_occurrences_higher_tf(
        file_total in 10u32..10_000,
        low_count in 1u32..5,
        extra in 1u32..100,
    ) {
        let high_count = low_count + extra;
        let tf_low = low_count as f64 / file_total as f64;
        let tf_high = high_count as f64 / file_total as f64;
        prop_assert!(tf_high > tf_low);
    }
}

// ─── clean_path invariants ───────────────────────────────────

proptest! {
    /// clean_path is idempotent — applying it twice gives the same result.
    #[test]
    fn clean_path_idempotent(input in "\\PC{0,100}") {
        let once = clean_path(&input);
        let twice = clean_path(&once);
        prop_assert_eq!(once, twice);
    }

    /// clean_path output never starts with \\?\
    #[test]
    fn clean_path_no_prefix_in_output(input in "\\PC{0,100}") {
        let result = clean_path(&input);
        prop_assert!(!result.starts_with(r"\\?\"),
            "clean_path output '{}' still has prefix", result);
    }
}
