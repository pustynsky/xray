# Validation Selection

Select checks from the touched behavior and changed contracts. These rows determine
which checks are required; they are not instructions to run every suite each time.
Do not install missing tools. Record unavailable checks and the affected coverage.
Once required checks pass, repeat only after relevant changes or new evidence.

## Rust

| Changed surface | Required evidence |
| --- | --- |
| Rust behavior or regression fix | Narrow `cargo test --bin xray <filter>` covering the changed scenario; Clippy for non-trivial production changes |
| Rust production code or Cargo build configuration | `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` |
| Feature wiring, parser registration, optional dependencies | `cargo check --no-default-features` and individual `lang-*` feature checks from current Cargo metadata |
| Persisted format or public shared contract | Relevant compatibility/reload tests and direct-consumer tests; broaden to full bin tests when affected callers cannot be isolated |
| Dependencies or license/advisory policy | Supplied or locally available audit/license evidence; record absence if the agent's whitelist prevents running a needed tool |
| Test-only additions | Relevant bin tests, assertion/fixture inspection, and counterfactual analysis |
| Docs/comments only | Verify changed claims against current code/docs; no build, Clippy, or unrelated tests |

Tests live in the binary target. Do not substitute a zero-test library run.
Do not serialize the full test suite by default. Do not run rustfmt.

## Scripts

| Changed surface | Required static checks |
| --- | --- |
| PowerShell code/tests | Parser check and `Invoke-ScriptAnalyzer -Severity Warning,Error` on touched files |
| bash | `bash -n` and `shellcheck -x` on touched scripts |
| Embedded Perl | Syntax check of the actual extracted snippets in a disposable location; inspect compile-time execution such as `BEGIN` blocks first |
| Only docs/comments | Verify changed claims; static checks and execution are not required |
| Only fixtures | Applicable round-trip regression; no unrelated linters |

Parser example (initialize `$errors` before calling):

```powershell
$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$null, [ref]$errors)
```

Enumerate current `scripts/**/test-*.ps1` suites and reconcile their scope
before selecting them. This table is a starting map, not a fixed suite count.
Run suites with `pwsh -NoProfile -File <suite>` after verifying temporary isolation.
For setup changes, run the applicable suites on Windows PowerShell 5.1 as well.

| Suite | Run when the diff touches |
| --- | --- |
| `test-mcp-setup.ps1` | Local CLI/VS Code configuration, tracked-file protection, excludes, trust, explicit legacy migration, restore/uninstall, worktrees, or standalone/in-memory setup |
| `test-detect-extensions.ps1` | Extension detection, suggested lists, confirmation prompts, or manual extension input |
| `test-setup-dependencies.ps1` | Public download fallback, dependency preflight, Git validation, or deprecated visibility arguments |

Include new suites whose scope matches the diff. Report only observed pass counts.
For a changed public invocation contract, test one affected documented launch form
in a disposable environment. A file-backed run does not prove an in-memory launch.
When PS 5.1/7 or Windows/Linux behavior differs in the affected path, require evidence
for those variants; otherwise state which variant remains unverified.

## Common Gates

- User-facing changes need CHANGELOG coverage and `scripts/check-product-names.ps1`.
  Inspect changed paths that the gate does not enumerate, including untracked and
  `.github` Markdown, for product-specific names.
- Review new Markdown justification and any relevant dependency-policy gates.
- Respect hooks and agent execution limits. Never bypass a gate to finish a review.
- Missing optional checks lower coverage as appropriate. Missing required checks
  block SHIP with a validation reason, not a fabricated BLOCKER/MAJOR finding.
- A pre-existing unrelated test failure is not a new regression. Report the failure
  and assess whether it prevents validation of the actual change.

## Reusing Supplied Evidence

Accept evidence only when it names the exact commands, exit codes, results, scope,
repository HEAD/base, and fingerprint of the same diff source being reviewed.

```text
VALIDATION_EVIDENCE
head: <commit SHA>
base: <base commit SHA for branch reviews, otherwise not-applicable>
diffSource: <working-tree | staged | branch range | branch range + working-tree fixes>
scope: <sorted paths>
diffSha256: <64 lowercase hex>
fixDiffSha256: <required only for a branch review with uncommitted fixes>
untrackedInputs: <none | sorted paths and SHA256 of each file's raw bytes>
commands:
- command: <exact command>
  exitCode: 0
  summary: <observed result>
END_VALIDATION_EVIDENCE
```

Compute the fingerprint over LF-joined Git diff output encoded as UTF-8. Use the
same sorted paths and diff source on both sides (`--cached` or the branch range
when applicable). For a working-tree review including both staged and unstaged
changes, use `git diff HEAD`, not plain `git diff`. Hash selected untracked inputs
separately; they are absent from all these tracked diffs. Check Git's exit code
before hashing. Set `$diffSource` and, for a branch range, `$base` and `$head` from
the recorded review state before running:

```powershell
$diffArgs = @(switch ($diffSource) {
    'working-tree' { @('HEAD') }
    'staged' { @('--cached') }
    'branch range' { @("$base..$head") }
    default { throw 'Choose one diff source; fingerprint a fix overlay separately' }
})
$lines = @(git diff @diffArgs --binary --no-ext-diff -- <sorted paths>)
if ($LASTEXITCODE -ne 0) { throw 'Cannot fingerprint the review diff' }
$text = $lines -join "`n"
$sha = [Security.Cryptography.SHA256]::Create()
try {
    $hash = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($text)))
    $hash.Replace('-', '').ToLowerInvariant()
} finally { $sha.Dispose() }
```

For a branch review with uncommitted fixes, `diffSha256` identifies the original
base-to-head range, and `fixDiffSha256` identifies the task-owned tracked fix
overlay against that HEAD. Include selected untracked fix files in `untrackedInputs`.
Both fingerprints and the combined file contents must match the validated state;
do not claim the original branch-only evidence validates the fix overlay.

A matching hash binds a supplied report to a patch; it does not authenticate the
report or prove that tests passed. Label it `SUPPLIED, HASH-MATCHED`, never reviewer-run.
Do not reuse if HEAD/base differs, relevant dependencies/configuration changed, or
untracked/unstaged inputs used by the check are outside the fingerprinted state.
For staged reviews, validation of a different working tree does not validate the
staged snapshot. If identity cannot be established, run the needed check safely or
record the gap. Matching evidence can replace a run, never code inspection.