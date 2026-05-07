# Installation Guide

End-to-end setup for using `xray` as an MCP server with your AI coding agent.

---

## Quick Setup (recommended)

The [`setup-xray.ps1`](../scripts/setup-xray.ps1) script automates the entire installation — download, extension detection, MCP config creation, and git protection — in one command:

```powershell
# Clone the repo (or just download the script)
git clone https://github.com/pustynsky/xray
cd xray

# Run setup for your target project
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject
```

**What it does:**

1. Downloads the latest `xray.exe` from [GitHub releases](https://github.com/pustynsky/xray/releases) to `%LOCALAPPDATA%\xray\`
2. Scans the target repository and detects file extensions (shows top-20, auto-suggests based on frequency)
3. Creates MCP configuration:
   - `.vscode/mcp.json` — for **VS Code GitHub Copilot Chat** (agent mode)
   - `.roo/mcp.json` — for **Roo Code** (optional, prompted with default N)
   - `.mcp.json` — for **GitHub Copilot CLI** (with `-EnableCopilotCli`)
4. Protects configs from accidental git push:
   - Tracked `.mcp.json` (shared repo case) → per-clone **git smudge/clean filter** so `git status` stays clean and `git pull` succeeds silently when upstream changes the file. See [Shared repo with a tracked `.mcp.json`](#shared-repo-with-a-tracked-mcpjson-smudgeclean-filter) below.
   - Tracked `.vscode/mcp.json` / `.roo/mcp.json` → `git update-index --skip-worktree` (local edits invisible to git; safe because these are typically untracked).
   - Untracked files → `.git/info/exclude` (local gitignore)

All xray tools are enabled by default **except** `xray_edit` (opt-in for safety).

**Options:**

```powershell
# Interactive mode (prompts for repo path)
.\scripts\setup-xray.ps1

# Skip download if xray.exe is already installed
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -SkipDownload

# Custom install location
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -InstallDir C:\Tools

# Overwrite existing configs without asking
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Force

# Use existing xray from a specific location
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -InstallDir "$HOME\.cargo\bin" -SkipDownload
```

After setup, **reopen the repo folder in VS Code** to activate the MCP server.

> **Cline users:** the script creates a config for Copilot by default. For Cline, see [Section 3c](#3c-cline-vs-code-extension--global-config-with-workspace-auto-detection) below for the global config format.
>
> **Roo Code users:** the `.roo/mcp.json` install path is currently disabled in `setup-xray.ps1`. The `-EnableRoo` switch is accepted as a no-op for backward compatibility but does nothing. To use xray with Roo, copy the `xray` entry from the generated `.mcp.json` into `.roo/mcp.json` by hand.

---

## Shared repo with a tracked `.mcp.json` (smudge/clean filter)

When the target repo already tracks `.mcp.json` upstream (the common case for shared engineering repos that publish team-wide MCP servers like a notes store, an issue-tracker bridge, Playwright, etc.), running the installer with `-EnableCopilotCli` wires a **per-clone git smudge/clean filter** instead of using `skip-worktree`.

### Install

```powershell
pwsh -File C:\path\to\xray\scripts\setup-xray.ps1 `
    -RepoPath C:\Repos\YourSharedRepo `
    -EnableCopilotCli `
    -Extensions cs,csproj,xml,manifestxml,json,md,ps1,sql,yaml,yml
```

### Behaviour after install

| Operation | Result |
| --------- | ------ |
| `git status` | clean — your local `xray` entry is invisible to git |
| `git pull` (upstream changed `.mcp.json`) | succeeds silently — no "would be overwritten" abort |
| `git add` / `git commit` of `.mcp.json` | produces an upstream-clean blob; your `xray.exe` path can never leak upstream |
| `git stash` / `git checkout` / `git reset --hard` / `git rebase` | preserves the entry in the working tree |

The trade-off: the `xray` entry inside `.mcp.json` is rendered as a single physical line tagged with a `_xrayMcpMarker` field. Other servers in the file keep their original formatting verbatim.

### Files written into the target repo

| Path | Purpose |
| ---- | ------- |
| `.git/xray-mcp/clean.sh` | per-clone filter that strips the xray entry on `git add` |
| `.git/xray-mcp/smudge.sh` | per-clone filter that re-injects it on checkout/pull |
| `.git/xray-mcp/snapshot.txt` | the byte-exact xray entry to inject |
| `.git/info/attributes` | `+ ".mcp.json filter=xray-mcp"` |
| `.git/config` | `+ [filter "xray-mcp"]` section, `required = false` |
| `.mcp.json` | one-line xray entry inserted as the first `mcpServers` member |

Nothing is written outside `.git/` and the `.mcp.json` working-tree file. No commits, no history rewrites.

### Uninstall — full rollback

```powershell
pwsh -File C:\path\to\xray\scripts\setup-xray.ps1 -RepoPath C:\Repos\YourSharedRepo -Uninstall
```

| Flag | Effect |
| ---- | ------ |
| `-DryRun` | preview the plan without making changes |
| `-KeepBinary` | leave `xray.exe` in `%LOCALAPPDATA%\xray\` |
| `-KeepBackups` | keep `.mcp.json.bak` files |

Uninstall removes every artifact the installer added — the filter scripts, the `.git/config` section, the `.git/info/attributes` line, the xray snapshot inside `.mcp.json`, and (on legacy installs) any `skip-worktree` / `assume-unchanged` flag and `.git/info/exclude` entries. After it runs, `git ls-files -v .mcp.json` shows `H` (no flags) and `git diff HEAD -- .mcp.json` is empty — the file is byte-identical to the upstream blob. The same uninstall flow also tears down xray entries from `.vscode/mcp.json`, `.roo/mcp.json`, and any other MCP host configs the installer touched on prior runs.

### Why a filter and not `skip-worktree`?

Earlier versions of the installer used `git update-index --skip-worktree`. That hides the local change from `git status` but causes `git pull` to abort with `error: Your local changes to .mcp.json would be overwritten by merge` whenever upstream touches the file — and recovery requires non-obvious manual steps (lift skip-worktree, stash, pull, pop, re-apply). Full rationale and the alternatives considered: [docs/mcp-filter-design.md](mcp-filter-design.md).

### Regression tests

The filter behaviour is covered by two PowerShell test suites in [scripts/mcp-filter/](../scripts/mcp-filter):

- `test-roundtrip.ps1` — byte-exact `clean → smudge → clean` round-trip across 5 JSON shapes plus a synthetic CRLF case
- `test-e2e.ps1` — full lifecycle in a temp clone: install → upstream change → pull → stash/pop → reset --hard → branch switch → uninstall, asserting `git status` clean and xray persistence at each step

Both run on Windows with Git for Windows (perl from `C:\Program Files\Git\usr\bin\` is the only external dependency).

---

## Manual Setup

If you prefer to set things up by hand, or need to configure Cline, follow the steps below.

> **Platform note:** Pre-built releases are currently published for **Windows x64** only. For Linux / macOS, build from source — `cargo build --release` (see README).

> **Tip — let the agent set it up for you:** once you have `xray.exe` on disk, you can also just open a chat in **GitHub Copilot Chat (agent mode)**, **Roo Code**, or **Cline** and ask:
> _"Install the xray MCP server for this workspace. Binary is at `C:\path\to\xray.exe`. I want extensions <list> indexed, with `--definitions` and `--watch` enabled."_
>
> The agent already knows the MCP config schema for its host (it edits `.vscode/mcp.json` / `.roo/mcp.json` / `cline_mcp_settings.json` itself) and will create or patch the file for you. Use this guide as a reference for the exact flags it should write.

---

### 1. Download the pre-built binary

1. Open the [GitHub releases page](https://github.com/pustynsky/xray/releases) — the newest build is always at the top. (Permalink to the latest: <https://github.com/pustynsky/xray/releases/latest>.)
2. Download `xray.exe`.
3. Move it to a stable location, for example:

   ```
   %LOCALAPPDATA%\Programs\xray\xray.exe
   ```

   (or any folder you control — just keep the path stable, MCP clients will reference it).

4. **(Recommended)** Add the folder to your `PATH` so MCP configs can use the bare command `xray` instead of an absolute path:

   ```powershell
   # PowerShell — user-scope PATH
   [Environment]::SetEnvironmentVariable(
     'Path',
     [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + "$env:LOCALAPPDATA\Programs\xray",
     'User')
   ```

   Open a new terminal afterwards (existing shells won't see the change).

5. Verify the install:

   ```powershell
   xray --version
   ```

> **SmartScreen note:** Windows may flag an unsigned `.exe` on first launch ("Windows protected your PC"). Click **More info → Run anyway**, or right-click the file → **Properties → Unblock → Apply** before first run. In practice this prompt does not always appear.

---

## 2. About the indexes (you usually don't run anything here)

**Short version:** skip this section on first install. The MCP server (`xray serve`, configured in step 3) builds and maintains all indexes automatically. There is no separate "install index" step.

What actually happens when `xray serve --watch --definitions` starts the first time:

1. Server's MCP endpoint is up immediately and answers `initialize` / `tools/list`.
2. In the background it scans `--dir`, builds the file-list, content (inverted) and definitions (AST) indexes, and writes them to disk.
3. Until that finishes, search tools return a friendly _"Index is being built, please retry"_ message instead of erroring.
4. From then on, `--watch` keeps everything fresh on every file change (< 1 sec per change).

So step 3 below is all you need for a working MCP setup.

### Where indexes are stored

All indexes live under a single per-user directory:

```
%LOCALAPPDATA%\xray\
```

They are shared between the CLI and the MCP server, and keyed by workspace root — so multiple projects coexist without conflict. Files written there:

| File | Built by | Used by |
|---|---|---|
| `*.file-list` | `serve` (or `xray index`) | `xray_fast` |
| `*.word-search` | `serve` (or `xray content-index`) | `xray_grep` |
| `*.code-structure` | `serve --definitions` (or `xray def-index`) | `xray_definitions`, `xray_callers` |
| `*.git-history` | background, on `serve` start | `xray_git_*`, `xray_branch_status` |

All files are LZ4-compressed. Safe to delete the whole folder — indexes will rebuild on next `serve` start.

### How long the first build takes

| Project size | Cold first-build time (rough) |
| --- | --- |
| Small (≤ a few thousand files) | a few seconds |
| Medium (~10–20K files) | ~10–20 sec |
| Large (~50K+ files, e.g. ~48K C# files) | **~25–50 sec** combined (content + AST definitions), depending on CPU thread count (see [benchmarks.md](benchmarks.md#build-times-across-machines)) |

Subsequent `serve` starts load the cached indexes from disk in **0.7–1.6 sec**. Incremental updates (with `--watch`) are **< 1 sec per changed file**.

### Optional: pre-warm the indexes via CLI

If you really want the very first MCP query to hit a warm index (e.g. you're scripting CI / first-impression demo), you can build the indexes ahead of time with the CLI — they'll be loaded by `serve` instead of rebuilt:

```powershell
xray content-index -d C:\Projects\MyApp -e cs,sql,csproj,xml --respect-git-exclude
xray def-index     -d C:\Projects\MyApp -e cs,sql                --respect-git-exclude
```

`--respect-git-exclude` honours `.gitignore` / `.git/info/exclude` so build artefacts (`bin/`, `obj/`, `node_modules/`, etc.) are skipped. Recommended for any git-tracked project. Add the same flag to your `xray serve` args (Section 3) for consistency.

For normal interactive use this is unnecessary — just go to step 3 and let `serve` do it.

---

## 3. Configure your AI agent

`xray serve` speaks MCP over **stdio**, so any MCP-compatible client works. Below are the three configurations the project author actively uses.

In all examples (workspace-scoped configs):

- `command` is `xray` if the binary is on `PATH`, otherwise the full absolute path (e.g. `C:\\Users\\you\\.cargo\\bin\\xray.exe` for `cargo install`-based installs, or `%LOCALAPPDATA%\\Programs\\xray\\xray.exe` for the pre-built binary).
- `--dir C:\\Repos\\MyApp` — **workspace root**. Point this at the folder you want xray to index. For workspace-scoped configs (`.vscode/mcp.json`, `.roo/mcp.json`) this is just the absolute path of the repo. **You can omit `--dir`** for a *global* config (single MCP entry shared across many projects) — `xray serve` then auto-detects the workspace from the MCP client's working directory / `roots`. This is what the Cline section below uses.
- `--ext rs md ps1` (or `--ext rs,md,ps1`) — **file extensions to index**. List every extension you want searchable: source code (`cs`, `ts`, `rs`, `sql`, …) plus configuration / docs you want the agent to find (`csproj`, `xml`, `config`, `json`, `yml`, `md`, `txt`, `ps1`). Both space-separated and comma-separated forms work.
- `--definitions` — **important.** Enables AST-based definition + caller indexes. Without this, `xray_definitions`, `xray_callers`, `xray_reindex_definitions` are unavailable.
- `--watch` — **important.** Enables filesystem watcher for incremental updates. Without it, indexes go stale after every edit and you have to call `xray_reindex` manually.
- `--respect-git-exclude` — **recommended for git-tracked projects.** Skips files ignored by `.gitignore` / `.git/info/exclude` (build artefacts, `node_modules/`, etc.) so they don't bloat the index or pollute search results.
- (optional) `--metrics`, `--debug-log` — extra diagnostics; safe to leave off for normal use.

### 3a. VS Code — GitHub Copilot Chat (agent mode)

Copilot Chat reads MCP configuration from a workspace-scoped `.vscode/mcp.json`. The schema uses `servers` (not `mcpServers`).

Create `.vscode/mcp.json` in your repository root:

```json
{
  "servers": {
    "xray": {
      "type": "stdio",
      "command": "xray",
      "args": [
        "serve",
        "--dir",
        "C:\\Projects\\MyApp",
        "--ext",
        "cs,sql,csproj,xml,config,json,md",
        "--watch",
        "--definitions"
      ]
    }
  },
  "inputs": []
}
```

Then:

1. Reload VS Code (or run **MCP: List Servers → xray → Restart**).
2. Open Copilot Chat in **Agent** mode.
3. Verify: ask _"Use xray_grep to find files containing HttpClient"_.

> **Note on the Copilot CLI agent:** MCP configuration for the standalone `copilot-cli` agent is a separate flow and not covered here yet.

### 3b. Roo Code (VS Code extension) — per-project config

Roo supports both global and project-scoped MCP. Project-scoped lives at `.roo/mcp.json` in the workspace root and uses the `mcpServers` schema with `alwaysAllow`.

Create `.roo/mcp.json`:

```json
{
  "mcpServers": {
    "xray": {
      "command": "xray",
      "args": [
        "serve",
        "--dir",
        "C:\\Projects\\MyApp",
        "--ext",
        "cs,sql,csproj,xml,config,json,md",
        "--definitions",
        "--watch"
      ],
      "alwaysAllow": [
        "xray_grep",
        "xray_fast",
        "xray_definitions",
        "xray_callers",
        "xray_help",
        "xray_info",
        "xray_reindex",
        "xray_reindex_definitions",
        "xray_git_history",
        "xray_git_diff",
        "xray_git_authors",
        "xray_git_activity",
        "xray_git_blame",
        "xray_branch_status"
      ],
      "disabled": false
    }
  }
}
```

Roo Code panel → **MCP Servers → Edit Project MCP** opens this file. After saving, restart the server from the same panel.

> **Note on `xray_edit`:** the example above intentionally **omits** `xray_edit` from `alwaysAllow` so the agent has to ask before each file edit. This matches the safer default used by `setup-xray.ps1`. Add `"xray_edit"` to the list only if you want unattended edits.

### 3c. Cline (VS Code extension) — global config with workspace auto-detection

Cline does not currently expose per-workspace MCP config — there is only a single global file:

```
%APPDATA%\Code\User\globalStorage\saoudrizwan.claude-dev\settings\cline_mcp_settings.json
```

To make one entry work across all your projects, **omit `--dir`** and let `xray serve` auto-detect the workspace from the MCP client's working directory / roots. The agent will issue one or two extra calls on first use to resolve and bind the workspace, then cache it.

Open the file via Cline panel → **MCP Servers → Configure MCP Servers**, and add:

```json
{
  "mcpServers": {
    "xray": {
      "type": "stdio",
      "command": "xray",
      "args": [
        "serve",
        "--definitions",
        "--watch"
      ],
      "autoApprove": [
        "xray_grep",
        "xray_fast",
        "xray_definitions",
        "xray_callers",
        "xray_help",
        "xray_info",
        "xray_reindex",
        "xray_reindex_definitions",
        "xray_git_history",
        "xray_git_diff",
        "xray_git_authors",
        "xray_git_activity",
        "xray_git_blame",
        "xray_branch_status"
      ],
      "disabled": false,
      "timeout": 60
    }
  }
}
```

> Schema differences between Roo and Cline: Roo uses `alwaysAllow`, Cline uses `autoApprove`. Otherwise the server block is identical. As with the Roo example, `xray_edit` is intentionally omitted — add it only if you want unattended edits.

---

## 4. Verify the MCP server is working

Three quick checks — do them in order, stop at the first one that fails:

1. **Server registered & alive.** In VS Code: open Command Palette → **MCP: List Servers** → you should see `xray` listed with a green dot. In Roo / Cline: open the **MCP Servers** panel; `xray` should be listed with no error badge. If it's red, click into it and read the stderr — most failures are a wrong `command` path or a missing `--dir`.
2. **Tools are exposed.** Ask the agent in plain English:

   > _"List the MCP tools available from the xray server."_

   You should see `xray_grep`, `xray_fast`, `xray_definitions`, `xray_callers`, `xray_edit`, `xray_info`, `xray_help`, plus the `xray_git_*` family. If only some appear, you likely forgot `--definitions`.
3. **Indexes are built and queryable.** Ask:

   > _"Call xray_info and show me the indexes."_

   Expect a JSON summary listing file-list, content, and (with `--definitions`) definition indexes — with sizes, timestamps, and a `workspaceStatus: "resolved"` field. If you instead see _"Index is being built, please retry"_, that's expected on first run for a large repo (see _"How long the first build takes"_ above) — wait ~30 sec and ask again.

Then do one real query, e.g.:

> _"Use xray_grep to find files containing HttpClient."_

If it returns ranked file paths with line numbers and scores in < 1 sec, you're done.

---

## 5. Updating to a new version

The MCP server holds an open handle on `xray.exe`, so the file is locked while VS Code is running. To upgrade:

```powershell
# 1. Stop all running xray instances (MCP servers + any CLI sessions)
Get-Process xray -EA SilentlyContinue | Stop-Process -Force

# 2. Replace the binary
Copy-Item .\xray.exe "$env:LOCALAPPDATA\Programs\xray\xray.exe" -Force

# 3. Restart the MCP server in your IDE
#    VS Code:  Command Palette → "MCP: List Servers" → xray → Restart
#    Roo/Cline: MCP Servers panel → Restart
```

To grab the latest build, see the [releases page](https://github.com/pustynsky/xray/releases) — the newest `xray.exe` is always at the top, or use [`/releases/latest`](https://github.com/pustynsky/xray/releases/latest) for a permalink.

---

## 6. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Agent says "tool not available" / `xray_*` missing | MCP server didn't start, or config not reloaded | Restart the MCP server in the IDE; check the server's stderr in the MCP output panel |
| `xray.exe` not found | Bare `command: "xray"` but binary not on `PATH` | Either add to `PATH` and reopen the IDE, or use the full absolute path in `command` |
| `--dir` errors / "directory not found" | Path with single backslashes in JSON | Escape backslashes: `"C:\\Projects\\MyApp"` |
| Tools work but return stale results | Index out of date (e.g. branch switch without `--watch`) | Ask agent to call `xray_reindex` and `xray_reindex_definitions` |
| "Index is being built, please retry" persists | Very large repo on cold start | Pre-build with `xray content-index -d ... -e ...` once |
| Upgrade copy fails with "file in use" | MCP server still holds the binary | See step 5 — kill all `xray` processes first |
| SmartScreen blocks `xray.exe` on first run | Unsigned binary | Right-click the file → **Properties → Unblock**, or **More info → Run anyway** at the prompt |

---

## See also

- [Releases](https://github.com/pustynsky/xray/releases) — always the latest `xray.exe` at the top
- [MCP Server Guide](mcp-guide.md) — full tools API, response schemas, policy / hint fields
- [CLI Reference](cli-reference.md) — all commands and flags
- [Architecture](architecture.md) — index types, watcher, git cache
