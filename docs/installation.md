# Installation Guide

End-to-end setup for using `xray` as an MCP server with your AI coding agent.

---

## Quick Setup (recommended)

The [`setup-xray.ps1`](../scripts/setup-xray.ps1) script automates the entire installation — download, extension detection, MCP config creation, and git protection — in one command.

### Requirements

- Windows x64 with Windows PowerShell 5.1+ or PowerShell 7+.
- Public releases download directly over HTTPS; GitHub CLI (`gh`) is not required. An authenticated `gh` installation is used only as a fallback for private or restricted repositories supplied through `-GithubRepo`.
- Git-managed operations, including restore, require working Git. A non-Git folder can still be configured, but setup reports that Git exclusion protection is unavailable.
- No bash/perl filter runtime is needed. New installations never install Git filters or hide tracked changes with index flags.
- VS Code and GitHub Copilot CLI do not need to be installed while running setup; the script only writes their selected config files.

Three ways to launch it. Pick whichever fits — they all run the same script and accept the same parameters.

### A1. One-liner (download + run inline)

Shortest. The script is fetched and executed in-memory; nothing is left on disk. The script will prompt for the target repository path; pass `-RepoPath <path>` to skip the prompt.

```powershell
& ([scriptblock]::Create((Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing).Content))
```

#### From Command Prompt (`cmd.exe`)

CMD has no PowerShell host, so launch the same installer through the built-in `powershell.exe`:

```cmd
powershell.exe -NoLogo -NoProfile -Command "& ([scriptblock]::Create((Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing).Content))"
```

Put installer parameters inside the outer double quotes. In CMD, wrap parameter values containing spaces in PowerShell single quotes, not additional double quotes:

```cmd
powershell.exe -NoLogo -NoProfile -Command "& ([scriptblock]::Create((Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing).Content)) -RepoPath 'C:\My Repo' -EnableVSCode"
```

### A2. Download-then-run

Recommended if you want to read the script before executing it. Saves to `%TEMP%`, runs from there.

```powershell
$tmp = Join-Path $env:TEMP 'setup-xray.ps1'
Invoke-WebRequest 'https://raw.githubusercontent.com/pustynsky/xray/main/scripts/setup-xray.ps1' -UseBasicParsing -OutFile $tmp
& $tmp
```

### A3. From a clone of this repo

Use this if you want to pin a specific commit/tag, or if you're iterating on the script locally.

```powershell
git clone https://github.com/pustynsky/xray
cd xray
.\scripts\setup-xray.ps1
```

> **Pin a release.** Replace `main` in the URLs above with a tag (e.g. `v0.4.0`) to lock the script to a specific release for reproducibility.
>
> **Target a specific repo non-interactively.** Pass `-RepoPath <path>` to skip the path prompt.
>
> **Select clients.** Both are configured by default. Add `-EnableCopilotCli` or `-EnableVSCode` to configure only the selected clients.
>
> **`-UseBasicParsing`.** Required on Windows PowerShell 5.1 (the default `powershell.exe` on Windows) — without it, `Invoke-WebRequest` uses the IE rendering engine which blocks with a security prompt and can dump the script's `.SYNOPSIS` block to the console. It's a no-op on PowerShell 7+.

**What it does:**

1. Downloads the latest `xray.exe` from [GitHub releases](https://github.com/pustynsky/xray/releases) to `%LOCALAPPDATA%\xray\`
2. Scans the target repository and detects file extensions (shows the top 20, selects frequent extensions, and retains detected structural files below the normal threshold)
3. Creates MCP configuration:
   - `.vscode/mcp.json` — for **VS Code GitHub Copilot Chat**
   - `.github/mcp.json` — for **GitHub Copilot CLI**
   - Roo Code (`.roo/mcp.json`) — **not** written; see the Roo note below
4. Keeps generated configs and backups local through Git's `info/exclude`, resolved with `git rev-parse --git-path info/exclude` for ordinary clones and worktrees. If repository rules re-include a config, setup creates a narrow, untracked per-directory ignore helper. The shared `.gitignore` is never modified.
5. Offers persistent Copilot CLI folder trust interactively. Enter means Yes. Existing trust and unrelated settings are preserved; `COPILOT_HOME` overrides the default home directory. Non-interactive runs and `-Force` skip granting trust.
6. If a root `.mcp.json` exists, creates a local `copilot-xray.cmd` in the repository root. The launcher explicitly loads `.github/mcp.json` alongside the root servers; the root config remains untouched.

The CLI definition enables all advertised Xray tools with `tools: ["*"]`. This is separate from CLI tool approval permissions. The VS Code definition has no `tools` field.

**Options:**

```powershell
# Interactive mode (prompts for repo path)
.\scripts\setup-xray.ps1

# Skip download if xray.exe is already installed
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -SkipDownload

# Custom install location
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -InstallDir C:\Tools

# Update local configs non-interactively, without granting trust or migration
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Force

# Use existing xray from a specific location
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -InstallDir "$HOME\.cargo\bin" -SkipDownload
```

After setup, **reopen the repo folder in VS Code** to activate the MCP server.

> **Cline users:** the script creates a config for Copilot by default. For Cline, see [Section 3c](#3c-cline-vs-code-extension--global-config-with-workspace-auto-detection) below for the global config format.
>
> **Roo Code users:** the `.roo/mcp.json` install path is disabled. `-EnableRoo` is accepted with a warning but does nothing. Use the manual Roo example below; its format differs from the CLI definition.

---

## Make Xray preferred for code discovery

MCP configuration makes tools available; it does not guarantee which tools an
agent selects. Xray already sends tool-selection instructions during MCP
initialization and reminders in responses. Add persistent client instructions
if you want to explicitly choose Xray-first discovery across sessions.

This is a manual, optional step. The setup script does not create or modify
agent instruction files. Folder trust and tool approval are separate settings;
neither establishes a tool preference.

### 1. Try it in the current chat

With Xray connected to the intended repository, send:

```text
Use Xray first for code discovery in this workspace: file lookup, content
search, symbol exploration, source reading, and caller/callee navigation.
If Xray is unavailable or cannot cover the operation, briefly explain why
before using an alternative. Keep existing editing, build, and test workflows.
```

This requests a preference for the current conversation. It does not save
instructions or guarantee that a new session or subagent inherits them.

### 2. Save the preference in your client's instructions

Start with repository scope. Choose one instruction format supported by the
client and agent harness you actually use:

| Client | Where to put the instruction block |
|---|---|
| VS Code GitHub Copilot | Append to `.github/copilot-instructions.md` at the repository root. Check that the selected harness loads it; discovery rules vary by harness. |
| GitHub Copilot CLI | Append to the repository's `.github/copilot-instructions.md` and start a new session in that repository. |
| Visual Studio GitHub Copilot | Append to `.github/copilot-instructions.md`. Enable custom instruction loading under **Tools > Options > GitHub > Copilot > Copilot Chat**; the exact settings layout varies by version. MCP connection setup is separate from this instruction step. |
| Other MCP clients | Use the client's documented persistent instruction mechanism. Use a root `AGENTS.md` only if that client supports it. |

See the official [VS Code custom instructions](https://code.visualstudio.com/docs/agent-customization/custom-instructions)
and [Visual Studio custom instructions](https://learn.microsoft.com/en-us/visualstudio/ide/copilot-chat-context)
documentation for client settings and discovery details.

Preserve existing instructions and reconcile conflicting tool preferences
before appending the block. Do not create duplicate copies in several formats
unless your clients require them. Repository instruction files may be tracked:
review the change before sharing it with your team. For a personal-only
preference, use your client's supported user-level instructions instead.
User-level instructions can affect other projects, so keep the workspace
condition in the block.

Copy this block into the chosen instruction file:

```markdown
<!-- xray-discovery-preference:start -->
## Code discovery with Xray

When Xray MCP is connected and serves the current workspace, use its tools
before built-in equivalents for code discovery:

- Use `xray_fast` for file and directory lookup.
- Use `xray_grep` for content search and matching-line context.
- Use `xray_definitions` for supported symbols, module structure, and source
  bodies. Use `includeBody=true` when reading a body.
- Use `xray_callers` for supported caller/callee navigation.

Respect the server's workspace scope, indexed extensions, and language support.
Use built-in alternatives when Xray is unavailable, the target is outside its
coverage, or the operation is unsupported. Briefly state the reason before
falling back. Do not bypass content exclusions or access restrictions.

This preference covers discovery only. Keep existing editing, build, and test
workflows. When delegating code discovery, include this preference and its
workspace scope in the delegated task rather than assuming inheritance.
<!-- xray-discovery-preference:end -->
```

The markers make the block easy to find and remove; they have no special meaning
to the client. This block does not change tool permissions or server-side
instructions, which also describe editing and Git tools.

### 3. Verify actual tool selection

Start a new chat or CLI session so the client loads the saved instructions.
Check that Xray is connected to the intended repository and that the instruction
file is loaded:

- **Copilot CLI:** run `/instructions` to list the instruction files discovered
  for the session.
- **VS Code Copilot:** check the response's **References** for applied
  instructions. Use **Chat: Open Customizations** to inspect instructions
  available to the selected harness.
- **Visual Studio Copilot:** check the response's **References** for applied
  repository instructions.

Discovering an instruction file does not prove the agent followed it. Verify
the tool calls separately.

Ask ordinary questions without naming Xray or prescribing a tool. Replace the
examples with files and symbols that exist in your repository:

| Request | Expected discovery calls |
|---|---|
| "Find files whose names contain OrderService." | `xray_fast` |
| "Find occurrences of HttpClient and show the surrounding lines." | `xray_grep` with `showLines=true` |
| "Show the body of OrderService.Submit and find its callers." | `xray_definitions` with `includeBody=true`, then `xray_callers`, where supported |

Inspect the actual tool-call trace, not just the agent's claim that it used Xray.
An explicit request such as "Use xray_grep" checks tool availability, not whether
the agent prefers it. Repeat a request in another fresh session to check
persistence. If discovery is delegated, check the subagent's calls as well.

Also check fallback with a permitted, non-sensitive target outside Xray's
coverage, or in a session with Xray disconnected. The agent should explain the
limitation before using an available alternative, not claim that an unavailable
index proves there are no matches.

Instructions guide model behavior; they do not enforce exclusive use. Strict
Xray-only operation requires client-side tool restrictions or enforcement
supported by the orchestrator. Neither this block nor MCP instructions override
higher-priority host instructions or security policies.

### Remove the preference

Delete only the block between `xray-discovery-preference:start` and
`xray-discovery-preference:end`, including the markers, from the instruction
file you chose. Preserve unrelated instructions, then start a new session.
Xray remains connected and continues to send its own MCP guidance. Removing this
block removes the saved user preference; it does not disable the server.

---

## Local Source Deployment

Use this after changing source in a local clone. It builds and deploys the binary only; run `setup-xray.ps1` separately to configure an MCP client or target repository.

```powershell
.\scripts\install-local.ps1
```

The script performs a locked release build, terminates all running `xray.exe` processes, verifies the staged binary, retries transient deployment locks, deploys to `%LOCALAPPDATA%\xray\xray.exe`, and verifies the installed hash and version. Restart the MCP host after it completes.

---

## Local MCP configuration and legacy migration

### Configuration and Git protection

CLI uses `mcpServers.xray` in `.github/mcp.json`, with `type: "stdio"` and
`tools: ["*"]`. VS Code uses `servers.xray` in `.vscode/mcp.json`, with the same
type, executable and arguments, plus the informational `"//"` string property.
The VS Code entry has no `tools` field.

Setup rebuilds only the Xray definition. Other servers and root settings remain.
The installer owns `command` and `args`; manual extra launch flags are replaced
on rerun. Invalid JSON, an incompatible container, or nesting beyond the
supported serialization depth of 100 is reported and left untouched, including
any existing backup. Use JSON rather than JSONC for consistent behavior on PowerShell 5.1
and 7.

Tracked target configs or backups are skipped, even with `-Force`.
`info/exclude` does not protect tracked files, and setup never untracks them.
If any selected client cannot be configured, the summary reports an incomplete
setup and exits nonzero. Successfully configured clients remain available.

The local ignore helper is needed when a higher-priority repository rule such
as `!**/.vscode/mcp.json` overrides a per-file exclude. Setup verifies both the
config and helper are ignored; it refuses a tracked helper or ineffective rule
instead of writing an unprotected config. New helper rules are anchored to that
directory, so they do not hide unrelated nested files with the same name.

### Copilot CLI trust

Workspace MCP discovery requires folder trust. Setup checks `trustedFolders`
in `$HOME\.copilot\config.json`, or under `COPILOT_HOME` when set. Existing trust,
including a trusted parent directory, needs no prompt. Otherwise interactive
setup asks with Yes as the default; declining leaves the config unchanged.
Non-interactive setup never grants trust, including with `-Force`.

Copilot-generated leading `//` comment headers are supported on PowerShell 5.1
and 7. An updated config is written as JSON without the header; the backup
retains the original comments.

Changes preserve unrelated settings, create a backup of an existing config,
and use an atomic write with a concurrent-change check. Malformed JSON or an
invalid trust list is reported without overwriting it or logging credentials.
Trust failure does not fail the MCP installation; the summary reports it
separately. This is not VS Code Workspace Trust or Git `safe.directory`.

Verify from the target repository:

```powershell
copilot mcp list
copilot mcp get xray
```

### Local CLI launcher when a root config exists

Copilot CLI 1.0.95 loads the root `.mcp.json` instead of `.github/mcp.json`
when both exist. It does not merge the two files, even when their server names
are different. Setup therefore creates `copilot-xray.cmd` in the repository
root when a root `.mcp.json` remains after any authorized migration.

```powershell
.\copilot-xray.cmd
.\copilot-xray.cmd --resume
```

Verify inside the launched session with `/mcp list` or `/mcp show xray`.
In CLI 1.0.95, the terminal subcommands `copilot mcp list` and `copilot mcp get`
do not include servers supplied through the session-only
`--additional-mcp-config` option, even when invoked through the launcher.

The launcher forwards arguments to Copilot, returns its exit code, and finds
`.github/mcp.json` relative to its own location. It does not embed a machine
or drive path. It passes `-C` to run the Copilot session from the repository
root without changing the caller's directory. This requires a CLI version
that supports `-C`, including the verified version 1.0.95.

Setup verifies that Git ignores the launcher before writing it. A tracked
or unrecognized launcher is never overwritten. If repository rules prevent
excluding this root file, setup leaves it unwritten and reports an incomplete
installation rather than changing the shared `.gitignore`. The summary also
shows the explicit fallback command:

```powershell
copilot --additional-mcp-config '@.\.github\mcp.json'
```

Without a root `.mcp.json`, ordinary `copilot` discovers `.github/mcp.json`
and no launcher is needed. A later setup removes a managed launcher if the
root config is gone. Selecting only VS Code does not create or remove it.

### Explicit legacy migration

Ordinary setup leaves the root `.mcp.json` untouched. It detects root Xray
entries, `_xrayMcpMarker`, old filters, attributes, filter directories, and
index flags. A root definition can override the new CLI entry, so leaving
legacy state behind produces a warning and an incomplete setup result.

Interactive setup asks before cleanup, with No as the default. Automation must
authorize it explicitly:

```powershell
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -MigrateLegacy -Force
```

`-MigrateLegacy` also authorizes removing a manually configured root server
named `xray`. Read the reported plan before granting consent. `-Force` alone
does not authorize root edits or clearing index flags.

After the selected new configs are ready, migration backs up the legacy config,
removes only the old Xray entry, clears authorized index flags, and removes
the corresponding filter registration, attributes and directory. Other
servers, settings and attributes are preserved. The old single-line snapshot
form is removed without reformatting the upstream config when possible.
The script never restores the whole root file from `HEAD` or an old backup.

New setup supports Git worktrees. **Legacy filter removal is refused while
multiple worktrees share its metadata.** Coordinate cleanup in a single-worktree
clone before retrying; setup does not edit other worktrees or tear down a filter
they may still depend on. Cleanup failures are reported without claiming a
successful migration.

`-GitVisibility Visible` and `Hidden` are deprecated aliases for the same
local-only setup, accepted with a warning. Neither recreates the old mechanism.
The [retired filter design](mcp-filter-design.md) is kept as historical context.

### Restore and uninstall

`-Restore` restores eligible local CLI and VS Code backups, not the root config.
Unchanged reruns do not rewrite configs or refresh backups.
Restore removes the managed local launcher if the restored CLI config has no
Xray entry or the root `.mcp.json` is gone. If launcher cleanup fails, eligible
CLI and VS Code backups are still restored; setup reports the cleanup failure
and exits nonzero.

```powershell
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Restore
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Uninstall -DryRun
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Uninstall -KeepBinary
```

Uninstall removes local Xray entries, preserves other servers, and deletes
an otherwise empty generated config and the managed, untracked CLI launcher.
The launcher's exclude rule is removed even if the file was deleted manually,
but kept while another linked worktree still has a launcher.
Dry runs leave the launcher unchanged. `-KeepBinary` retains the executable;
`-KeepBackups` retains local backups. Existing configs with other servers stay
locally excluded. If a config changes during uninstall, setup refuses to delete
or overwrite it and retains the backup. Directory-local ignore helpers remain
in place so existing unrelated rules are preserved.
It does not revoke Copilot trust or modify Roo/global configs.
Legacy cleanup during uninstall still requires migration consent:

```powershell
.\scripts\setup-xray.ps1 -RepoPath C:\Repos\MyProject -Uninstall -MigrateLegacy -KeepBinary
```

### Regression tests

[test-mcp-setup.ps1](../scripts/test-mcp-setup.ps1) exercises local configs,
Git ignore precedence, tracked and malformed inputs, atomic writes, trust,
worktrees, standalone execution and explicit legacy cleanup in isolated
temporary directories. It never modifies actual user or repository configs.
[Extension](../scripts/test-detect-extensions.ps1) and
[dependency](../scripts/test-setup-dependencies.ps1) regression tests remain.
CI runs all three on Windows PowerShell 5.1 and PowerShell 7.

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
