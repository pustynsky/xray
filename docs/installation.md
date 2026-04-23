# Installation Guide

End-to-end setup for using `xray` as an MCP server with your AI coding agent.

This guide covers the **pre-built binary** path, which is the recommended option for users who only want to run the MCP server (no Rust toolchain required). If you want to build from source instead, see the [Quick Start in README](../README.md#installation).

> **Platform note:** Pre-built releases are currently published for **Windows x64** only. For Linux / macOS, build from source — `cargo build --release` (see README).

> **Tip — let the agent set it up for you:** once you have `xray.exe` on disk, you can also just open a chat in **GitHub Copilot Chat (agent mode)**, **Roo Code**, or **Cline** and ask:
> _"Install the xray MCP server for this workspace. Binary is at `C:\path\to\xray.exe`. I want extensions <list> indexed, with `--definitions` and `--watch` enabled."_
>
> The agent already knows the MCP config schema for its host (it edits `.vscode/mcp.json` / `.roo/mcp.json` / `cline_mcp_settings.json` itself) and will create or patch the file for you. Use this guide as a reference for the exact flags it should write.

---

## 1. Download the pre-built binary

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
|---|---|
| Small (≤ a few thousand files) | a few seconds |
| Medium (~10–20K files) | ~30 sec |
| Large (~50K+ files) | **~1 min** for content + AST definitions combined |

Subsequent `serve` starts load the cached indexes from disk in **0.7–1.6 sec**. Incremental updates (with `--watch`) are **< 1 sec per changed file**.

### Optional: pre-warm the indexes via CLI

If you really want the very first MCP query to hit a warm index (e.g. you're scripting CI / first-impression demo), you can build the indexes ahead of time with the CLI — they'll be loaded by `serve` instead of rebuilt:

```powershell
xray content-index -d C:\Projects\MyApp -e cs,sql,csproj,xml
xray def-index     -d C:\Projects\MyApp -e cs,sql
```

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
        "xray_edit",
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
        "xray_edit",
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

> Schema differences between Roo and Cline: Roo uses `alwaysAllow`, Cline uses `autoApprove`. Otherwise the server block is identical.

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
