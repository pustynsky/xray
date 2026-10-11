# Script Review Hazards

Read only the sections relevant to the diff. Treat these as failure hypotheses,
not proof that the current implementation has a defect.

## Safety Boundaries

- Validate only in disposable repositories and install directories under the
  resolved temporary root. Check both paths before executing a mutating test.
- Never run the installer on a real working tree, kill live Xray processes, install
  tooling, use elevation, change global Git configuration, or download a binary.
- Manual installer repros need `-SkipDownload`, explicit temporary `-RepoPath` and
  `-InstallDir`, and no `-KillRunning`. Inspect suites for equivalent isolation.
- Cleanup must validate containment of its target and avoid following junctions
  or symlinks into a real repository or installation.

## PowerShell

- Support Windows PowerShell 5.1 and PowerShell 7 where the touched behavior differs.
  Check `$PSNativeCommandUseErrorActionPreference`, `$ErrorActionPreference`, and
  `$LASTEXITCODE` together. Stderr redirection does not guarantee a failed native
  probe is harmless. Do not assume the same preference defaults on every host.
- Distinguish null from an empty collection; `@($null).Count` is one. Inspect
  empty JSON-container checks and object/array shapes after serialization.
- Parenthesize expressions such as `-not ($values -contains 'x')`.
- Check trailing newlines and encoding. Windows PowerShell 5.1 UTF-8 output can
  include a BOM; file consumers and byte-preservation contracts decide what is valid.
- Re-serializing upstream-managed JSON can change layout or shape. Ordinary setup
  must refuse tracked targets; untracked JSON edits must preserve other servers,
  date-looking strings, nulls, and array shapes.
- Check variables referenced across disabled blocks and early returns for missing
  initialization. Normalize paths before comparing Git and PowerShell output.
- Installation should be idempotent. Inspect duplicate excludes, ignore helpers,
  backups, partial installs, and explicit restore behavior.
- For `$MyInvocation`, `$PSScriptRoot`, `$PSCommandPath`, or `$args` changes, inspect
  every relevant use under both file-backed and in-memory invocation.

## Local Setup And Legacy Migration

- New installs must not register clean/smudge filters, alter root `.mcp.json`,
  or set index flags. `Visible` and `Hidden` are deprecated local-only aliases.
- Verify actual Git ignore behavior, not just appended exclude lines. Tracked
  negations can require a narrow untracked directory-local ignore helper.
- Legacy cleanup requires `-MigrateLegacy` or affirmative interactive consent.
  `-Force` must not grant it. Unresolved legacy state must not look like success.
- Preserve unrelated servers and attributes. Never restore the entire root config
  from `HEAD` or a backup to remove one legacy entry.
- Clear both old index flags only during authorized migration. Shared filter
  metadata belongs under the Git common directory; refuse shared teardown when
  multiple worktrees exist.
- Copilot trust must respect `COPILOT_HOME` and existing parent trust. Noninteractive
  setup must not grant trust, and uninstall must not revoke it.

## Public Modes And Postconditions

For installer changes, derive affected invocation forms from current README/help,
not the requester's test selection. Mark each as tested, argued, or unverified:

- In-memory scriptblock or `iex` execution: a staged file run is not equivalent;
  command-path variables may be null.
- Downloaded standalone file: verify no sibling script dependency.
- Clone-mode script: verify the same local configuration contract.
- Install from and run inside a linked worktree: verify common-dir resolution.

For each affected mode, consider clean/prior/legacy/partial installs, tracked and
untracked configs, both config targets, explicit client selection, legacy migration,
deprecated visibility arguments, trust consent, submodules,
restore, uninstall, and dry-run only where the changed invariant reaches them.
Check the promised result: Git cleanliness, exact client schema, exclude/attribute
state, preserved unrelated servers, and safe restore, not just exit zero.

## Security And Repository Gates

- Verify download source, asset selection, and integrity where changed. Do not
  execute fetched content during review or silently install after integrity failure.
- Treat repo/install paths and extension arguments as untrusted command inputs.
  Check PS 5.1 quoting and the generated MCP command/arguments.
- Recursive deletion must prove containment before acting and handle reparse points.
- User-facing changes require CHANGELOG coverage and the product-name gate;
  inspect changed files outside that gate's enumerated paths separately.
  New Markdown needs justification; preserve script line-ending attributes.