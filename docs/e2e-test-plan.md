# E2E Test Plan — MOVED

This document has been decomposed into modular files for better navigation and maintainability.

**See [`docs/e2e/README.md`](e2e/README.md)** for the full test plan index.

## Files

| File | Scope |
|------|-------|
| [docs/e2e/README.md](e2e/README.md) | Overview, configuration, automation script |
| [docs/e2e/cli-tests.md](e2e/cli-tests.md) | CLI commands: fast, grep, index, info, cleanup |
| [docs/e2e/mcp-grep-tests.md](e2e/mcp-grep-tests.md) | MCP `xray_grep`: substring, phrase, truncation |
| [docs/e2e/mcp-definitions-tests.md](e2e/mcp-definitions-tests.md) | MCP `xray_definitions`: body, hints, auto-correct, code stats |
| [docs/e2e/mcp-callers-tests.md](e2e/mcp-callers-tests.md) | MCP `xray_callers`: call trees, DI, overloads |
| [docs/e2e/mcp-fast-edit-tests.md](e2e/mcp-fast-edit-tests.md) | MCP `xray_fast`, `xray_edit` |
| [docs/e2e/git-tests.md](e2e/git-tests.md) | Git tools, cache, blame, branch status |
| [docs/e2e/language-tests.md](e2e/language-tests.md) | SQL, TypeScript, Angular parser tests |
| [docs/e2e/infrastructure-tests.md](e2e/infrastructure-tests.md) | Server protocol, async, shutdown, compression, memory, routing |
