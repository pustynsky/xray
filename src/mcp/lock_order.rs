//! Lock ordering regression harness.
//!
//! Enforces the global acquisition order documented in
//! [`docs/concurrency.md`](../../docs/concurrency.md) §"Lock Ordering Contract":
//!
//! ```text
//! 1. ctx.index       (ContentIndex)
//! 2. ctx.def_index   (DefinitionIndex)
//! 3. ctx.file_index  (FileIndex)
//! 4. ctx.git_cache   (GitHistoryCache)
//! ```
//!
//! Use [`mark_content_acquired`] / [`mark_content_released`] and
//! [`assert_can_acquire_def`] in the hot paths that take both locks on the
//! same call stack. On debug builds (including `cargo test`) a violation
//! triggers a `debug_assert!` panic; in release builds it logs a `warn!` so
//! production traffic is never killed by the harness itself.
//!
//! This is intentionally a *detection* harness, not a runtime enforcement
//! mechanism — proper enforcement happens at the Rust type-system level by
//! threading `Option<&ContentIndex>` through the handler call graph (see the
//! [`super::handlers::definitions`] module for an example).

use std::cell::Cell;

thread_local! {
    static CONTENT_READ_HELD: Cell<u32> = const { Cell::new(0) };
    static DEF_READ_HELD: Cell<u32> = const { Cell::new(0) };
}

/// Record that the current thread is about to acquire a read guard on
/// `ctx.index` (content index). Must be paired with
/// [`mark_content_released`] exactly once.
pub fn mark_content_acquired() {
    CONTENT_READ_HELD.with(|c| c.set(c.get().saturating_add(1)));
}

/// Record that a previously acquired content read guard has just been
/// dropped. Safe to call when the counter is zero (no-op).
pub fn mark_content_released() {
    CONTENT_READ_HELD.with(|c| c.set(c.get().saturating_sub(1)));
}

/// Record that the current thread is about to acquire a read guard on
/// `ctx.def_index`. Must be paired with [`mark_def_released`] exactly once.
pub fn mark_def_acquired() {
    DEF_READ_HELD.with(|c| c.set(c.get().saturating_add(1)));
}

/// Record that a previously acquired def read guard has just been dropped.
pub fn mark_def_released() {
    DEF_READ_HELD.with(|c| c.set(c.get().saturating_sub(1)));
}

/// Assert that acquiring `ctx.index` right now would not violate lock
/// ordering. Call this *before* `ctx.index.read()` / `.write()` on any call
/// stack that may already hold `ctx.def_index`.
///
/// Violation is a programmer error (AB/BA hazard — see docs/concurrency.md).
pub fn assert_can_acquire_content() {
    DEF_READ_HELD.with(|d| {
        let held = d.get();
        debug_assert!(
            held == 0,
            "LOCK ORDER VIOLATION: tried to acquire ctx.index while holding \
             ctx.def_index on this thread (count={held}). See docs/concurrency.md \
             §'Lock Ordering Contract'. Acquire ctx.index FIRST, then \
             ctx.def_index."
        );
        if held != 0 {
            tracing::warn!(
                def_held = held,
                "Lock-order violation: acquired ctx.index while ctx.def_index \
                 still held on this thread (AB/BA hazard, see docs/concurrency.md)"
            );
        }
    });
}

/// Assert that acquiring `ctx.def_index` right now is fine. Always safe —
/// def is level 2 and any level ≤ 2 may be held first. Provided for
/// symmetry so callers can instrument both sides.
pub fn assert_can_acquire_def() {
    // No assertion — def is allowed after content (level 1 → 2).
    // This helper exists so call sites can symmetrically bookend both locks
    // (mark_content_* and mark_def_*), and so future levels can be added
    // without touching the call sites.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard that resets thread-local counters at start and end of a test,
    /// so tests in this module do not pollute each other (tests may share
    /// threads in the default test runner).
    struct ResetGuard;
    impl ResetGuard {
        fn new() -> Self {
            CONTENT_READ_HELD.with(|c| c.set(0));
            DEF_READ_HELD.with(|d| d.set(0));
            Self
        }
    }
    impl Drop for ResetGuard {
        fn drop(&mut self) {
            CONTENT_READ_HELD.with(|c| c.set(0));
            DEF_READ_HELD.with(|d| d.set(0));
        }
    }

    #[test]
    fn content_then_def_is_ok() {
        let _g = ResetGuard::new();
        mark_content_acquired();
        assert_can_acquire_def();
        mark_def_acquired();
        mark_def_released();
        mark_content_released();
    }

    #[test]
    fn def_alone_is_ok() {
        let _g = ResetGuard::new();
        mark_def_acquired();
        mark_def_released();
    }

    #[test]
    fn content_alone_is_ok() {
        let _g = ResetGuard::new();
        mark_content_acquired();
        mark_content_released();
    }

    #[test]
    #[should_panic(expected = "LOCK ORDER VIOLATION")]
    fn def_then_content_panics_in_debug() {
        let _g = ResetGuard::new();
        mark_def_acquired();
        assert_can_acquire_content();
    }
}
