use std::collections::HashSet;
use std::ops::Range;
use std::slice;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) enum ScopeSelection {
    All {
        file_count: usize,
    },
    Filtered {
        file_ids: Vec<u32>,
        membership_words: Vec<u64>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScopeResolutionStrategy {
    All,
    ExactMap,
    LinearScan,
}

impl ScopeResolutionStrategy {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::ExactMap => "exactMap",
            Self::LinearScan => "linearScan",
        }
    }
}

#[derive(Debug)]
pub(crate) struct ResolvedFileScope {
    selection: ScopeSelection,
    total_files: usize,
    strategy: ScopeResolutionStrategy,
    resolution_duration: Duration,
}

impl ResolvedFileScope {
    pub(crate) fn resolve<F>(
        files: &[String],
        has_execution_filter: bool,
        exact_file_id: Option<u32>,
        mut predicate: F,
    ) -> Self
    where
        F: FnMut(&str) -> bool,
    {
        let started = Instant::now();
        if !has_execution_filter {
            return Self {
                selection: ScopeSelection::All {
                    file_count: files.len(),
                },
                total_files: files.len(),
                strategy: ScopeResolutionStrategy::All,
                resolution_duration: started.elapsed(),
            };
        }

        let (file_ids, strategy) = if let Some(file_id) = exact_file_id {
            let file_ids = files.get(file_id as usize)
                .filter(|path| predicate(path))
                .map(|_| vec![file_id])
                .unwrap_or_default();
            (file_ids, ScopeResolutionStrategy::ExactMap)
        } else {
            let file_ids = files.iter().enumerate()
                .filter(|(_, path)| predicate(path))
                .map(|(file_id, _)| {
                    u32::try_from(file_id).expect("content index file_id exceeds u32")
                })
                .collect();
            (file_ids, ScopeResolutionStrategy::LinearScan)
        };

        let mut membership_words = vec![0; files.len().div_ceil(64)];
        for &file_id in &file_ids {
            let file_id = file_id as usize;
            membership_words[file_id / 64] |= 1_u64 << (file_id % 64);
        }

        Self {
            selection: ScopeSelection::Filtered {
                file_ids,
                membership_words,
            },
            total_files: files.len(),
            strategy,
            resolution_duration: started.elapsed(),
        }
    }

    pub(crate) fn contains(&self, file_id: u32) -> bool {
        let file_id = file_id as usize;
        if file_id >= self.total_files {
            return false;
        }
        match &self.selection {
            ScopeSelection::All { .. } => true,
            ScopeSelection::Filtered {
                membership_words,
                ..
            } => membership_words[file_id / 64] & (1_u64 << (file_id % 64)) != 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        match &self.selection {
            ScopeSelection::All { file_count } => *file_count,
            ScopeSelection::Filtered { file_ids, .. } => file_ids.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn is_all(&self) -> bool {
        matches!(self.selection, ScopeSelection::All { .. })
    }

    pub(crate) fn iter_ids(&self) -> ScopeFileIdIter<'_> {
        match &self.selection {
            ScopeSelection::All { file_count } => ScopeFileIdIter::All(0..*file_count),
            ScopeSelection::Filtered { file_ids, .. } => {
                ScopeFileIdIter::Filtered(file_ids.iter())
            }
        }
    }

    pub(crate) fn intersect_candidate_ids(&self, candidates: &HashSet<u32>) -> Vec<u32> {
        if candidates.len() < self.len() {
            let mut file_ids: Vec<u32> = candidates.iter()
                .copied()
                .filter(|file_id| self.contains(*file_id))
                .collect();
            file_ids.sort_unstable();
            return file_ids;
        }

        self.iter_ids()
            .filter(|file_id| candidates.contains(file_id))
            .collect()
    }

    pub(crate) fn skipped_by_scope(&self) -> usize {
        self.total_files.saturating_sub(self.len())
    }

    pub(crate) fn total_files(&self) -> usize {
        self.total_files
    }

    pub(crate) fn strategy(&self) -> ScopeResolutionStrategy {
        self.strategy
    }

    pub(crate) fn resolution_duration(&self) -> Duration {
        self.resolution_duration
    }
}

/// Intersects two strictly ascending, duplicate-free definition ID slices.
pub(crate) fn intersect_sorted_candidate_ids(
    candidates: &[u32],
    scoped_ids: &[u32],
) -> Vec<u32> {
    debug_assert!(candidates.windows(2).all(|pair| pair[0] < pair[1]));
    debug_assert!(scoped_ids.windows(2).all(|pair| pair[0] < pair[1]));

    const BINARY_SEARCH_RATIO: usize = 8;

    if candidates.len().saturating_mul(BINARY_SEARCH_RATIO) < scoped_ids.len() {
        return candidates.iter()
            .copied()
            .filter(|candidate| scoped_ids.binary_search(candidate).is_ok())
            .collect();
    }
    if scoped_ids.len().saturating_mul(BINARY_SEARCH_RATIO) < candidates.len() {
        return scoped_ids.iter()
            .copied()
            .filter(|candidate| candidates.binary_search(candidate).is_ok())
            .collect();
    }

    let mut intersection = Vec::with_capacity(candidates.len().min(scoped_ids.len()));
    let mut candidate_pos = 0;
    let mut scoped_pos = 0;
    while candidate_pos < candidates.len() && scoped_pos < scoped_ids.len() {
        match candidates[candidate_pos].cmp(&scoped_ids[scoped_pos]) {
            std::cmp::Ordering::Less => candidate_pos += 1,
            std::cmp::Ordering::Greater => scoped_pos += 1,
            std::cmp::Ordering::Equal => {
                intersection.push(candidates[candidate_pos]);
                candidate_pos += 1;
                scoped_pos += 1;
            }
        }
    }
    intersection
}


pub(crate) enum ScopeFileIdIter<'a> {
    All(Range<usize>),
    Filtered(slice::Iter<'a, u32>),
}

impl Iterator for ScopeFileIdIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::All(range) => range.next().map(|file_id| {
                u32::try_from(file_id).expect("content index file_id exceeds u32")
            }),
            Self::Filtered(file_ids) => file_ids.next().copied(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::All(range) => range.size_hint(),
            Self::Filtered(file_ids) => file_ids.size_hint(),
        }
    }
}

impl ExactSizeIterator for ScopeFileIdIter<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(count: usize) -> Vec<String> {
        (0..count).map(|file_id| format!("src/file_{file_id}.rs")).collect()
    }

    #[test]
    fn unfiltered_scope_uses_all_without_membership_words() {
        let files = paths(3);
        let scope = ResolvedFileScope::resolve(&files, false, None, |_| {
            panic!("unfiltered scope must not evaluate the predicate")
        });

        assert!(scope.is_all());
        assert_eq!(scope.len(), 3);
        assert_eq!(scope.iter_ids().collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(matches!(
            scope.selection,
            ScopeSelection::All { file_count: 3 }
        ));
        assert_eq!(scope.strategy(), ScopeResolutionStrategy::All);
    }

    #[test]
    fn filtered_scope_builds_sorted_ids_and_compact_membership() {
        let files = paths(130);
        let scope = ResolvedFileScope::resolve(&files, true, None, |path| {
            path.ends_with("file_0.rs")
                || path.ends_with("file_64.rs")
                || path.ends_with("file_129.rs")
        });

        assert_eq!(scope.iter_ids().collect::<Vec<_>>(), vec![0, 64, 129]);
        assert!(scope.contains(0));
        assert!(scope.contains(64));
        assert!(scope.contains(129));
        assert!(!scope.contains(63));
        assert!(!scope.contains(130));
        assert_eq!(scope.skipped_by_scope(), 127);
        match &scope.selection {
            ScopeSelection::Filtered {
                membership_words,
                ..
            } => assert_eq!(membership_words.len(), 3),
            ScopeSelection::All { .. } => panic!("expected filtered scope"),
        }
    }

    #[test]
    fn filtered_scope_can_be_empty() {
        let files = paths(2);
        let scope = ResolvedFileScope::resolve(&files, true, None, |_| false);

        assert!(scope.is_empty());
        assert!(!scope.is_all());
        assert!(!scope.contains(0));
        assert_eq!(scope.skipped_by_scope(), 2);
    }

    #[test]
    fn exact_lookup_checks_the_legacy_predicate() {
        let files = paths(3);
        let included = ResolvedFileScope::resolve(&files, true, Some(1), |_| true);
        let excluded = ResolvedFileScope::resolve(&files, true, Some(1), |_| false);

        assert_eq!(included.iter_ids().collect::<Vec<_>>(), vec![1]);
        assert!(excluded.is_empty());
        assert_eq!(included.strategy(), ScopeResolutionStrategy::ExactMap);
    }

    #[test]
    fn candidate_intersection_is_sorted_and_ignores_unknown_ids() {
        let files = paths(130);
        let candidates = HashSet::from([129, 64, 0, 999]);
        let all = ResolvedFileScope::resolve(&files, false, None, |_| true);
        let filtered = ResolvedFileScope::resolve(&files, true, None, |path| {
            path.ends_with("file_0.rs") || path.ends_with("file_64.rs")
        });

        assert_eq!(all.intersect_candidate_ids(&candidates), vec![0, 64, 129]);
        assert_eq!(filtered.intersect_candidate_ids(&candidates), vec![0, 64]);
    }

    #[test]
    fn sorted_candidate_intersection_handles_adaptive_size_ratios() {
        let dense: Vec<u32> = (0..100).collect();
        let sparse = vec![1, 50, 99, 200];

        assert_eq!(
            intersect_sorted_candidate_ids(&dense, &sparse),
            vec![1, 50, 99]
        );
        assert_eq!(
            intersect_sorted_candidate_ids(&sparse, &dense),
            vec![1, 50, 99]
        );
        assert_eq!(
            intersect_sorted_candidate_ids(&[1, 3, 5], &[2, 3, 4]),
            vec![3]
        );
    }

    #[test]
    fn resolver_matches_legacy_predicate_for_every_file() {
        let files = vec![
            "src/main.rs".to_string(),
            "src/generated.rs".to_string(),
            "tests/main.rs".to_string(),
            "README.md".to_string(),
        ];
        let legacy_predicate = |path: &str| path.starts_with("src/") && !path.contains("generated");
        let expected: Vec<u32> = files.iter().enumerate()
            .filter_map(|(file_id, path)| legacy_predicate(path).then_some(file_id as u32))
            .collect();
        let scope = ResolvedFileScope::resolve(&files, true, None, legacy_predicate);

        assert_eq!(scope.iter_ids().len(), expected.len());
        assert_eq!(scope.iter_ids().collect::<Vec<_>>(), expected);
        for (file_id, path) in files.iter().enumerate() {
            assert_eq!(scope.contains(file_id as u32), legacy_predicate(path));
        }
        assert_eq!(scope.strategy(), ScopeResolutionStrategy::LinearScan);
        assert_eq!(scope.total_files(), files.len());
        assert_eq!(scope.strategy().as_str(), "linearScan");
        assert!(scope.resolution_duration() <= Duration::from_secs(1));
    }
}

