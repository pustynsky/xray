#!/usr/bin/env bash
# clean filter for .mcp.json (xray MCP per-clone setup).
# Reads enriched .mcp.json from stdin, writes canonical (upstream-only) form
# to stdout by deleting any line carrying the _xrayMcpMarker sentinel.
#
# Idempotent: an input with no marker passes through byte-identical.
# Designed to be the exact inverse of smudge.sh on smudge-produced content.
#
# Implementation note: uses perl (bundled with Git for Windows) instead of
# sed because sed on Git for Windows normalizes CRLF -> LF, breaking
# byte-exact round-trip on Windows clones with CRLF-stored .mcp.json.
# perl with `:raw` binmode preserves bytes verbatim on all platforms.
#
# Installed as: .git/xray-mcp/clean.sh
# Wired via:    [filter "xray-mcp"] clean = .git/xray-mcp/clean.sh
#
# DO NOT EDIT BY HAND. setup-xray.ps1 manages this file.

set -eu
exec perl -e '
    binmode STDIN, ":raw";
    binmode STDOUT, ":raw";
    while (<STDIN>) {
        print unless /_xrayMcpMarker/;
    }
'
