#!/usr/bin/env bash
# smudge filter for .mcp.json (xray MCP per-clone setup).
# Reads canonical (upstream-only) .mcp.json from stdin, writes enriched form
# (with the local xray entry injected) to stdout.
#
# Strategy: insert the snapshot line as the FIRST entry inside mcpServers,
# immediately after the opening brace. This avoids matching the closing
# brace, which would require brace-counting that is not safe against `{`
# and `}` characters appearing inside JSON string values (common in args).
#
# Behavior:
#   * snapshot.txt missing             -> passthrough (filter no-op)
#   * input already contains marker    -> passthrough (defensive)
#   * mcpServers opens at end of line  -> inject after that line
#       - empty mcpServers (next non-blank line is `}` or `},`) -> inject without trailing comma
#       - non-empty mcpServers                                  -> inject with trailing comma
#   * any other shape (inline `{}`, single-line full mcpServers, missing key) -> passthrough
#
# Round-trip property (verified by test-roundtrip.ps1):
#   clean(smudge(canonical)) == canonical for every fixture in fixtures/.
#
# Implementation note: uses perl (bundled with Git for Windows) instead of
# awk because awk on Git for Windows normalizes CRLF -> LF even with
# BINMODE=3, breaking byte-exact round-trip on Windows clones with
# CRLF-stored .mcp.json. perl with `:raw` binmode preserves bytes verbatim
# on all platforms.
#
# `filter.required = false` is set so that any failure here (including
# missing bash on PATH) degrades to passthrough rather than aborting git.
#
# Installed as: .git/xray-mcp/smudge.sh
# Wired via:    [filter "xray-mcp"] smudge = .git/xray-mcp/smudge.sh
#
# DO NOT EDIT BY HAND. setup-xray.ps1 manages this file.

set -eu

snapshot_path="$(dirname "$0")/snapshot.txt"

# Snapshot missing -> passthrough.
if [ ! -r "$snapshot_path" ]; then
    exec cat
fi

exec perl -e '
    use strict;
    use warnings;
    binmode STDIN,  ":raw";
    binmode STDOUT, ":raw";

    # Read snapshot (single line; strip any trailing CR/LF).
    my $snapshot_path = $ARGV[0];
    open my $sf, "<:raw", $snapshot_path or do { print while <STDIN>; exit 0 };
    my $snap = do { local $/; <$sf> };
    close $sf;
    $snap =~ s/[\r\n]+\z//;

    # Slurp entire input.
    my $all = do { local $/; <STDIN> };
    $all = "" unless defined $all;

    # Defensive: if the marker is already present, passthrough byte-exact.
    if ($all =~ /_xrayMcpMarker/) {
        print $all;
        exit 0;
    }

    # Detect dominant line separator (\r\n vs \n).
    # If we see at least one \r\n in the input, use it; else \n; else \n.
    my $sep = ($all =~ /\r\n/) ? "\r\n" : "\n";

    # Split into lines preserving separators. Use split with limit=-1 so
    # trailing empty strings are kept; structure is text,sep,text,sep,...
    my @parts = split /(\r?\n)/, $all, -1;

    my @out;
    my $injected = 0;
    my $i = 0;
    my $n = scalar @parts;
    while ($i < $n) {
        my $text = $parts[$i];
        my $line_sep = ($i + 1 < $n) ? $parts[$i + 1] : "";
        push @out, $text;
        push @out, $line_sep if $line_sep ne "";

        if (!$injected && $text =~ /"mcpServers"\s*:\s*\{\s*$/) {
            # Peek next text segment (skip blank lines).
            my $j = $i + 2;  # next text index after sep
            while ($j < $n && $parts[$j] =~ /^\s*$/ && (($j + 1 < $n) ? $parts[$j + 1] ne "" : 0)) {
                # Push the blank line through.
                push @out, $parts[$j];
                push @out, $parts[$j + 1];
                $j += 2;
            }
            my $next = ($j < $n) ? $parts[$j] : "";
            my $trimmed = $next;
            $trimmed =~ s/^\s+//;
            $trimmed =~ s/\s+$//;
            my $is_empty_servers = ($trimmed eq "}" || $trimmed eq "}," || $trimmed eq "} ,");

            my $line_to_emit = $is_empty_servers ? $snap : $snap . ",";
            push @out, $line_to_emit;
            push @out, $line_sep;  # match same line separator as opening line
            $injected = 1;

            # Continue from $j (we already pushed blank lines).
            $i = $j;
            next;
        }

        $i += 2;  # advance past text + sep
    }

    print join("", @out);
' "$snapshot_path"
