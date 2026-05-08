# Design: Per-clone xray entry in `.mcp.json` via git smudge/clean filter

Status: proposed
Owner: setup-xray.ps1
Audience: maintainers of `setup-xray.ps1` and reviewers

## Problem

`setup-xray.ps1 -EnableCopilotCli` adds an `xray` MCP server entry to the
canonical project-level config file `.mcp.json` at the repository root.
This is the only path GitHub Copilot CLI reads for project-scoped MCP
servers. (`.github/copilot/mcp.json` and `.github/mcp.json` are not read,
verified empirically against the official Copilot CLI.)

In repositories where `.mcp.json` is **tracked upstream** (the common case
for shared engineering repos that already publish MCP servers like a
notes store, an issue-tracker bridge, Playwright, etc.), our local
addition of an `xray` entry creates a tension between three user
requirements:

1. The `xray` entry is **per-machine** (it embeds an absolute path to the
   user's local `xray.exe`) and must never be committed.
2. The user must not see `.mcp.json` as a permanently dirty file in
   `git status`. Dirty status is intrusive, hides real local edits, and
   trains users to ignore it.
3. When upstream adds, removes, or modifies a server in `.mcp.json`,
   `git pull` must succeed silently. The user must not be required to
   know any non-standard recovery procedure (no manual stash dance, no
   custom `-Recover` command, no half-broken merge state).

The current implementation uses `git update-index --skip-worktree` on
`.mcp.json`. This satisfies (1) and (2) but breaks (3): when upstream
modifies `.mcp.json`, `git pull` aborts with

```
error: Your local changes to the following files would be overwritten by merge:
        .mcp.json
Please commit your changes or stash them before you merge.
Aborting
```

The user's local file looks clean to `git status` (because of
skip-worktree), so the error message is actively confusing — there are
no visible local changes to stash. Recovery requires non-obvious steps:
lift skip-worktree, stash, pull, pop, re-apply skip-worktree.

`assume-unchanged` was tested as an alternative and produces the same
abort behavior. No git index flag delivers all three requirements
simultaneously.

## Goals

* `git status` is clean immediately after install and remains clean
  through arbitrary upstream changes to `.mcp.json`.
* `git pull` succeeds silently regardless of whether upstream changed
  `.mcp.json`. The user never has to invoke a custom command to keep
  their working tree in sync.
* `git add` / `git commit` of `.mcp.json` (intentional or accidental)
  produces an upstream-clean commit. The user's xray entry is physically
  unable to leak upstream.
* `git stash`, `git checkout BRANCH`, `git reset --hard`, and
  `git rebase` all preserve the user's xray entry in the working tree
  after they complete.
* The xray entry remains visible to MCP clients (Copilot CLI, Roo,
  VS Code Copilot) — the MCP-visible content is unchanged from today.
* No modifications to the `xray` Rust crate or binary. No new release
  artifacts. The mechanism is a per-clone, locally-installed shell
  script.
* Uninstall (`setup-xray.ps1 -Uninstall`) leaves the repository in a
  state indistinguishable from a fresh clone.

## Non-goals

* Cross-machine portability of the per-clone configuration. Each clone
  is configured independently by re-running `setup-xray.ps1`.
* Recovery from user-induced damage to files inside `.git/`. If the
  user manually deletes our scripts or snapshot from `.git/`, they
  re-run `setup-xray.ps1`.
* Protecting against upstream renaming the `mcpServers` JSON key or
  otherwise restructuring the file in incompatible ways. In that case
  the filter degrades to passthrough and the user re-runs install.

## Solution

Install a **git smudge/clean filter** for `.mcp.json` in each clone
where `setup-xray.ps1 -EnableCopilotCli` is executed. The filter is a
pair of shell scripts living under `.git/xray-mcp/`. Configuration is
written to per-clone files only (`.git/info/attributes` and
`.git/config`), neither of which is tracked.

### What git stores vs what the user sees

```
                          git index                 working tree
                          (committed)                (your disk)
                          ─────────────              ─────────────
       smudge:            upstream JSON      ───>    upstream JSON
                          (no xray)                  + xray block
                                                     between markers

       clean:             upstream JSON      <───    upstream JSON
                          (no xray)                  + xray block
                                                     between markers
```

The clean filter is byte-for-byte the inverse of the smudge filter on
content the smudge filter produced. Round-trip identity is guaranteed
because the filter operates on textual sentinel markers (not on parsed
JSON), and the markers are uniquely chosen so `sed` deletion and
re-insertion are exact inverses.

### Sentinel marker

Our injected content is **a single physical line** inside the
`mcpServers` object. The line contains the entire xray entry, plus
one extra field `_xrayMcpMarker` whose value is a unique sentinel
string. Example shape (line wrapped here for readability; in the file
it is one line):

```
    "xray":{"type":"stdio","command":"C:\\Tools\\xray\\xray.exe","args":["mcp"],"_xrayMcpMarker":"managed-by-setup-xray.ps1-do-not-edit"},
```

Why a single-line, JSON-valid form rather than JSONC comment markers:

* No assumption about MCP-client tolerance for JSON comments. Strict
  JSON parsers reject `/* ... */`. We never need to find out which
  clients are strict.
* `sed '/_xrayMcpMarker/d'` deletes exactly that line and leaves
  everything else byte-identical. The clean filter is one sed line.
* The `_xrayMcpMarker` field is an extra property on the xray server
  entry. MCP server schemas accept additional fields per the JSON
  Schema convention; even strict implementations leave the entry
  loadable because `command`/`type`/`args` are valid.
* The marker string `managed-by-setup-xray.ps1-do-not-edit` is
  improbable to collide with any user content.

Trade-off: the entry is on one long line. Pretty-printing is sacrificed
for round-trip stability (`perl -p` operates on whole lines). The user
does not edit this line by hand — `setup-xray.ps1` owns it.

### Files installed per clone

| Path | Tracked? | Purpose |
|---|---|---|
| `.mcp.json` | yes (upstream owns it) | working tree contains upstream + xray block; index contains upstream only |
| `.git/info/attributes` | no | `.mcp.json filter=xray-mcp` |
| `.git/config` (`[filter "xray-mcp"]` section) | no | `clean = bash "$(git rev-parse --git-common-dir)/xray-mcp/clean.sh"`, `smudge = bash "$(git rev-parse --git-common-dir)/xray-mcp/smudge.sh"`, `required = false` |
| `.git/xray-mcp/smudge.sh` | no | bash + `exec perl` (`:raw` binmode); reads canonical from stdin, injects xray block, writes enriched to stdout |
| `.git/xray-mcp/clean.sh` | no | bash + `exec perl` (`:raw` binmode); reads enriched from stdin, strips xray block, writes canonical to stdout |
| `.git/xray-mcp/snapshot.txt` | no | the literal text block to inject (regenerated on every install run) |

The filter command is stored as `bash "$(git rev-parse --git-common-dir)/xray-mcp/<name>.sh"` rather than a bare script path because:

1. **Executable-bit portability.** Git invokes filter commands via `sh -c '<cmd>'`. A bare script path requires the executable bit, which Windows filesystems don't carry, `IO.File.WriteAllText` doesn't set, and `git update-index --chmod=+x` can't apply (filter scripts are inside `.git/`, never tracked). Explicit `bash <path>` works on every git platform without any filesystem permission dance.
2. **Linked-worktree compatibility.** In a linked worktree (`git worktree add`), the worktree-rooted `.git` is a *file* (containing `gitdir: ...`), not a directory — a bare `.git/xray-mcp/...` path resolves to nothing. `git rev-parse --git-common-dir` returns the SHARED main gitdir for both primary and linked worktrees, so the filter command resolves correctly in either case. (`--git-dir` would NOT work for linked worktrees: it returns the per-worktree dir at `.git/worktrees/<name>/`, where the scripts do NOT live.) The `$(...)` is single-quoted into git config so PowerShell does NOT expand it; git stores the literal string, and `sh -c <command>` evaluates the substitution at filter-invocation time.

### Filter behavior

`smudge.sh`:

1. If `snapshot.txt` is missing → emit stdin unchanged (passthrough).
2. If stdin already contains the `_xrayMcpMarker` substring (defensive
   against double-smudge in edge cases) → emit unchanged.
3. Otherwise → `exec perl -e ...` reads stdin in `:raw` binmode (preserves
   CRLF and trailing-newline-or-not byte-exactly), detects the dominant
   line separator, finds the line that opens the `mcpServers` object,
   and emits the snapshot line immediately after it. If `mcpServers`
   already contains entries, the snapshot ends with a comma; if it's
   empty, no comma is appended. If `mcpServers` is fully on one line
   as `{}`, smudge cannot safely inject and falls through to passthrough.
4. If the perl script cannot find an injection point (no `mcpServers`
   key, inline-empty, malformed) → emit stdin unchanged. The user notices
   xray is missing from Copilot CLI and re-runs `setup-xray.ps1`.

`clean.sh`:

1. `exec perl -e 'binmode :raw; ...'` reads stdin and emits every line
   that does NOT contain the `_xrayMcpMarker` substring.
2. If no marker was present → output equals input. Idempotent and
   byte-exact (CRLF preserved).

`required = false` is critical. If bash is not on `PATH`, or
`.git/xray-mcp/` was wiped, git treats filter failure as passthrough
rather than aborting all operations on `.mcp.json`. This trades silent
loss of the xray entry for never breaking git for the user. The user
notices a missing xray entry in their Copilot CLI session, re-runs
`setup-xray.ps1`, and is back in business.

### `setup-xray.ps1` install flow (Copilot CLI section)

1. Resolve absolute path to `xray.exe`.
2. If `.mcp.json` is tracked by git (filter strategy):
   * Generate the canonical xray snapshot line (`{ "xray": { ... }, "_xrayMcpMarker": ... }`).
   * Backup to `.mcp.json.bak`.
   * Detect any pre-existing legacy xray block (from old skip-worktree-based installs) and remove it for clean re-install.
   * Walk the file by lines (preserving its dominant CRLF/LF separator) and inject the snapshot line immediately after the line that opens the `mcpServers` object. Empty-`mcpServers` case omits the trailing comma; non-empty appends one.
3. If `.mcp.json` does NOT exist OR is untracked, fall through to the legacy `ConvertTo-Json` path (no filter installed, no snapshot, plain JSON merge).
4. Copy filter scripts from `scripts/mcp-filter/{clean.sh,smudge.sh}` into the resolved git common dir at `<git-common-dir>/xray-mcp/` (LF-normalized; otherwise CRLF in the script bodies would break heredocs and `set -e` constructs). Scripts are NOT marked executable; they're invoked through an explicit `bash` interpreter (see step 6).
5. Write `<git-common-dir>/xray-mcp/snapshot.txt` containing the exact snapshot text the smudge filter must inject.
6. Set `.git/config` filter section, with each `git config` write checked for non-zero exit:
   ```
   [filter "xray-mcp"]
       clean    = bash "$(git rev-parse --git-common-dir)/xray-mcp/clean.sh"
       smudge   = bash "$(git rev-parse --git-common-dir)/xray-mcp/smudge.sh"
       required = false
   ```
   The `$(git rev-parse --git-common-dir)` substitution makes the path resolve correctly inside linked worktrees (where `.git` is a file pointing at the per-worktree dir, but the filter scripts live in the shared common dir). If any `git config` write fails (e.g., locked `.git/config`), `Install-McpFilter` warns with the exit code and returns `$false` so the caller can take the fail-closed path described below.
7. Append `.mcp.json filter=xray-mcp` to `<git-common-dir>/info/attributes` (idempotent, replaces any prior `.mcp.json` entry). The attribute does NOT include `text eol=lf`: the perl-based filter preserves CRLF byte-exact, and forcing LF in our local index when upstream's HEAD blob contains CRLF would create a permanent staged-renormalization marker in `git status`.
8. Run `git add --renormalize .mcp.json`, with `$LASTEXITCODE` checked. After this step the index reflects the canonical (upstream-only) form and `git status` is clean.
9. Print summary including the marker hint so anyone reading the file knows what the line is. The `_xrayMcpMarker` field doubles as a discoverability hint: searching for it points back to `setup-xray.ps1`.

**Fail-closed behavior for tracked `.mcp.json`.** If `Install-McpFilter` returns `$false` (any `git config` write failed, missing `bash`, missing source scripts, etc.) AND `.mcp.json` is tracked by git, `setup-xray.ps1` calls `Uninstall-McpFilter` to roll back any partial filter artifacts, emits a warning explaining the refusal, sets `$writeCopilotCli = $false`, and skips Copilot-CLI configuration for the run. It does NOT fall back to the legacy `ConvertTo-Json` + `git update-index --skip-worktree` path — doing so would re-introduce the silent-rebase / pull-abort hazard that the filter migration was designed to eliminate. Other clients (`.vscode/mcp.json`) continue to configure normally. Untracked `.mcp.json` installs are unaffected and continue to use the legacy plain-JSON path.

### `setup-xray.ps1 -Uninstall` flow

1. Run the clean filter manually (the perl marker-strip path) to remove
   the snapshot line from `.mcp.json` (so we don't depend on git
   operations succeeding for the strip).
2. `git update-index --no-skip-worktree .mcp.json` (defensive, in case
   a legacy install left the bit set).
3. Remove the `.mcp.json filter=xray-mcp` line from
   `<git-common-dir>/info/attributes` (preserve any other lines).
4. Remove `[filter "xray-mcp"]` section from `.git/config`
   (`git config --remove-section filter.xray-mcp`). `$LASTEXITCODE` is
   checked: if removal fails (e.g., locked `.git/config`), the function
   returns `'error'` and warns rather than silently leaving a
   filter section that points at scripts that are about to be deleted.
5. Delete `<git-common-dir>/xray-mcp/` directory.
6. Run `git add --renormalize .mcp.json` so the now-cleaned working
   tree matches the index.
7. Optionally delete `.mcp.json.bak` (`-KeepBackups` to skip).
8. Optionally delete `xray.exe` from `$InstallDir` (`-KeepBinary` to skip).
9. Optionally show what would be done without acting (`-DryRun`).
9. Optionally show what would be done without acting (`-DryRun`).

After uninstall the repository is byte-for-byte indistinguishable from
a fresh clone (modulo `.bak` files and the binary, both controlled by
the user via flags).

## Behavior matrix

| Scenario | Result |
|---|---|
| `git status` immediately after install | clean |
| `git diff` on `.mcp.json` | empty |
| `git pull` when upstream did not change `.mcp.json` | silent, working tree unchanged |
| `git pull` when upstream added/changed/removed a server in `.mcp.json` | silent, working tree contains upstream's new content + our xray block |
| `git add .mcp.json && git commit` | commit contains upstream-clean version (no xray entry) |
| `git stash` then `git stash pop` | xray block restored automatically |
| `git checkout other-branch` then `git checkout main` | xray block present after each checkout |
| `git reset --hard origin/main` | xray block restored automatically |
| Bash unavailable on the user's machine | filter no-ops; `.mcp.json` working tree contains upstream-only; user re-runs install |
| User wipes `.git/xray-mcp/` manually | filter no-ops; same recovery as above |
| Upstream renames `mcpServers` to something else | smudge cannot find injection point; passthrough; user re-runs install which detects the new structure |

## Risks and mitigations

**Round-trip stability.** The filter must satisfy
`clean(smudge(canonical)) == canonical` byte-for-byte for any canonical
input the user might pull from upstream. Mitigation: the smudge inserts
the block *between* lines and the clean removes those exact lines via
range delete. We never reformat the surrounding upstream content.
A property-style E2E test verifies this on a representative corpus
(empty `.mcp.json`, single-server, multi-server, with trailing
newlines, with CRLF, with BOM).

**Bash availability on Windows.** Git for Windows installs bash in
`C:\Program Files\Git\usr\bin\bash.exe` and adds it to git's own
exec-path, so any user with git on Windows has bash available to git
filters. Linux and macOS have bash natively. We do not depend on bash
being on the user's interactive `PATH` — git invokes filters using its
own resolution.

**Stale snapshot after `xray.exe` is moved.** If the user reinstalls
`xray.exe` to a different `$InstallDir`, the snapshot still references
the old path. Mitigation: `setup-xray.ps1` is the single source of
truth for the snapshot. Running it again regenerates the snapshot and
re-renormalizes `.mcp.json`. We document this in `--help`.

**`required = false` hides snapshot loss.** If `.git/xray-mcp/` is
deleted, the user gets the upstream-only `.mcp.json` and silently
loses xray in Copilot CLI. We accept this trade per the user's
explicit preference: silent xray loss > broken git. Discoverability
relies on the user noticing xray is missing in their next Copilot CLI
session; recovery is `setup-xray.ps1 -EnableCopilotCli`.

**Inline-empty `mcpServers`.** If upstream ships
`"mcpServers": {}` on a single line, smudge cannot inject without
rewriting brace whitespace. Mitigation: passthrough; the user
re-runs install which writes the file with multi-line `mcpServers`
from scratch. We document this edge case in install summary.

## What is removed by this design

* The `git update-index --skip-worktree` code path in `setup-xray.ps1`
  for `.mcp.json`. Replaced by the filter.
* The `.git/info/exclude` entry for `.mcp.json` (only relevant in the
  untracked-`.mcp.json` case). Still relevant for `.vscode/mcp.json`
  and `.roo/mcp.json` which are typically untracked and which we
  intentionally do not put behind a filter (they are not tracked
  upstream, so there is nothing to merge with).
* Any need for a `-Recover` command. The mechanism makes recovery
  automatic on every `git pull`.

## What is added by this design

* `-Uninstall` mode (covered above).
* The pair of shell scripts and the `.git/xray-mcp/` directory layout.
* A single-line xray entry in `.mcp.json` carrying the
  `_xrayMcpMarker` sentinel field.

## Out of scope

* A general-purpose "merge my snippet into a tracked file" framework.
  This design intentionally hardcodes the `xray` server name and the
  set of supported container shapes (`mcpServers` for Copilot CLI,
  `servers` for VS Code). Generalizing later is possible but not now.

## Extension: tracked `.vscode/mcp.json`

The same hazard class applies to `.vscode/mcp.json` whenever a shared
repo tracks it (some teams check in shared MCP servers for their VS
Code Copilot Chat workflow). The original "out of scope" note above
assumed `.vscode/mcp.json` was always untracked; field experience
proved otherwise (a tracked-upstream `.vscode/mcp.json` aborted
`git pull` in exactly the same way that motivated the original
filter migration).

The fix mirrors the `.mcp.json` design with one parameterization:

* Filter name: `xray-vscode-mcp` (separate `<git-common-dir>/xray-vscode-mcp/`
  directory and separate `[filter "xray-vscode-mcp"]` config section
  so the two installs are independent — installing/uninstalling one
  must never affect the other).
* Attribute path: `.vscode/mcp.json filter=xray-vscode-mcp`.
* Container key (the JSON object that holds server entries):
  `servers` instead of `mcpServers`. This is the only schema
  difference between the two file shapes.
* Snapshot entry shape: `{"type":"stdio","command":"...","args":[...],"_xrayMcpMarker":...}`
  (note `type:"stdio"` is required by the VS Code schema; `env:{}` is
  omitted because VS Code does not require it and including it would
  diverge from upstream conventions).

### How the parameterization is implemented

`smudge.sh` takes the container key as its first positional argument:

```bash
exec bash "$(git rev-parse --git-common-dir)/xray-mcp/smudge.sh" mcpServers
exec bash "$(git rev-parse --git-common-dir)/xray-vscode-mcp/smudge.sh" servers
```

Inside `smudge.sh`, the argument is validated against
`/^[A-Za-z_][A-Za-z0-9_]*$/` before being spliced into a perl regex
via `qr/"\Q$container_key\E"\s*:\s*\{\s*$/`. Defense-in-depth: the
PowerShell-side `Install-McpFilter` function also `[ValidateSet]`s the
key to `'mcpServers'` or `'servers'` so untrusted callers cannot
inject arbitrary regex even before reaching the bash layer. With no
argument, smudge defaults to `mcpServers` for backward compatibility
with the original install (older `.git/config` filter sections
written before this extension still work after the script update).

`clean.sh` is unchanged: it strips any line containing the literal
`_xrayMcpMarker` substring, regardless of which container the entry
lives in. One source file, two filter installs, two container shapes.

### Per-clone files for the second filter

| Path | Tracked? | Purpose |
|---|---|---|
| `.vscode/mcp.json` | yes (upstream owns it) | working tree contains upstream + xray block; index contains upstream only |
| `.git/info/attributes` (extra line) | no | `.vscode/mcp.json filter=xray-vscode-mcp` |
| `.git/config` (`[filter "xray-vscode-mcp"]` section) | no | `clean = bash "$(git rev-parse --git-common-dir)/xray-vscode-mcp/clean.sh"`, `smudge = bash "$(git rev-parse --git-common-dir)/xray-vscode-mcp/smudge.sh" servers`, `required = false` |
| `.git/xray-vscode-mcp/{smudge.sh,clean.sh,snapshot.txt}` | no | independent copy of the same source scripts; refreshing one filter never touches the other's directory |

### Install/uninstall flow

Identical to the `.mcp.json` flow above, with `Install-McpFilter` /
`Uninstall-McpFilter` invoked twice with different `-FilterName`,
`-ContainerKey`, `-AttributePath`, and `-McpRelPath` arguments. Each
call's `git config` writes are independently exit-code-checked, and
the same fail-closed rollback applies per file: a tracked
`.vscode/mcp.json` whose filter install fails causes the VS Code
section of the install to abort with rollback (rather than fall back
to `ConvertTo-Json` + `skip-worktree`, which would re-introduce the
original hazard). The other client's install proceeds independently.

### Tests

* `scripts/mcp-filter/test-roundtrip.ps1` — round-trip property
  `clean(smudge(canonical)) == canonical` for both container shapes
  including CRLF and the no-args backward-compat path. 11 fixtures.
* `scripts/mcp-filter/test-vscode-tracked.ps1` — full lifecycle
  reproduction of the original `.vscode/mcp.json` pull-abort scenario
  proving the filter prevents it. Mirrors `test-e2e.ps1` for the
  Copilot CLI side. 36 assertions covering install, upstream changes
  + `git pull`, `git stash`/`pop`, `git reset --hard`, branch
  switches, and clean uninstall.
