# Bug: `test_deeply_nested_no_stack_overflow` flakes under full-suite parallel run

**Date discovered:** 2026-04-26
**Severity:** Minor (CI noise; no production impact)
**Reporter:** Stas (during AC-4 grep prefilter implementation)
**Affected branch:** `main` (commit `6d27dbe`) — pre-existing, NOT introduced by AC-4

## Symptom

```text
test definitions::tests_xml::test_deeply_nested_no_stack_overflow ... FAILED

thread 'definitions::tests_xml::test_deeply_nested_no_stack_overflow' panicked at
  src\definitions\definitions_tests_xml.rs:551:5:
Deeply nested XML must never cause a hard error; got Some(TreeSitterReturnedNone)

test result: FAILED. 2314 passed; 1 failed; 0 ignored; 0 measured; finished in 59.58s
```

The test asserts that `parse_xml_on_demand_with_warnings(<2500-deep XML>, "test.xml")`
returns `Ok(_)`. Under parallel `cargo test --bin xray` it intermittently returns
`Err(TreeSitterReturnedNone)` instead.

## Reproduction

```powershell
# Reproduces ~50% of runs on Windows (tested on commit 6d27dbe with 16 logical cores)
cargo test --bin xray
```

Does **NOT** reproduce in isolation:

```powershell
cargo test --bin xray test_deeply_nested_no_stack_overflow  # always passes
```

## Root cause hypothesis

`tree-sitter-xml` returns `None` from `Parser::parse()` when it cannot allocate
a sufficiently large internal buffer or when its watchdog (timeout / memory
ceiling) trips. Under parallel test execution, the 2500-level-deep XML
fixture (~25 KiB on the wire, but exponentially more state during parse)
runs concurrently with hundreds of other tests competing for the same
allocator and CPU, increasing the probability of either path firing.

The test catches the resulting `Err(TreeSitterReturnedNone)` as a "hard
error" violation, but the production handler treats this case as a
graceful fallback (returns an empty definitions list with a typed warning).
So the assertion is **stricter than the production contract**.

## Why this is NOT an AC-4 regression

Verified by stashing the AC-4 working tree and running the full suite on
clean `main`:

| Branch | Pass | Fail | Total |
|---|---|---|---|
| `main` (baseline) | 2314 | 1 (this test) | 2315 |
| `main` + AC-4 working tree | 2334 | 1 (this test) | 2335 |

Same test, same panic message, same flakiness rate. AC-4 added 20 new
unit tests (all passing) and zero changes to `definitions::tests_xml`.

## Suggested fixes (in order of preference)

1. **Loosen the test contract** (~3 LOC). Replace the strict `is_ok()` with
   "either `Ok(_)` OR `Err(TreeSitterReturnedNone)`" — both outcomes are
   acceptable per the production handler's error path. The test's
   *original intent* was to prevent stack overflow / panic, not to assert
   `tree-sitter` always succeeds on adversarial input.

2. **Reduce DEPTH** from 2500 to 1500. Still well past our 1024 tripwire
   (the actual unit under test) but reduces tree-sitter memory pressure.

3. **Serialize via `serial_test::serial`** to remove parallel-allocator
   contention. Adds a dependency, slows the suite slightly.

4. **Pin to a single-thread test runner** for `tests_xml` only (would
   require a separate test binary). Heavier than this bug warrants.

## Recommended action

Apply fix #1 — the test as written checks the wrong thing relative to
the production contract. The test name says "no stack overflow"; the
assertion should match.

## References

- Test: [src/definitions/definitions_tests_xml.rs:531](../../src/definitions/definitions_tests_xml.rs#L531)
- Production handler error type: `XmlParseError::TreeSitterReturnedNone`
- Discovery context: AC-4 implementation, full-suite verification step before commit-reviewer
