<#
.SYNOPSIS
    Analyzes an exported Roo Code MCP transcript and produces a compact analytical report.

.DESCRIPTION
    Parses a Roo Code markdown export file, extracts MCP tool call episodes,
    computes tool quality scorecard, and generates JSON + Markdown reports.

.PARAMETER InputFile
    Path to the Roo Code markdown export file.

.PARAMETER OutputJson
    Path for the JSON output report. Default: <InputFile>.report.json

.PARAMETER OutputMd
    Path for the Markdown output report. Default: <InputFile>.report.md

.EXAMPLE
    pwsh -File scripts/analyze-transcript.ps1 -InputFile session.md
    pwsh -File scripts/analyze-transcript.ps1 -InputFile session.md -OutputJson report.json -OutputMd report.md
#>

param(
    [Parameter(Mandatory = $true)]
    [string]$InputFile,

    [string]$OutputJson,
    [string]$OutputMd
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# --- Defaults ---
if (-not $OutputJson) { $OutputJson = "$InputFile.report.json" }
if (-not $OutputMd) { $OutputMd = "$InputFile.report.md" }

# --- Read file ---
if (-not (Test-Path $InputFile)) {
    Write-Error "Input file not found: $InputFile"
    exit 1
}
$raw = Get-Content -Path $InputFile -Raw -Encoding UTF8
$fileName = Split-Path $InputFile -Leaf

# ============================================================
# STEP 1: Split into turns
# ============================================================

# Split on --- separator lines (horizontal rules between turns)
# Each turn starts with **User:** or **Assistant:**
$turnBlocks = [regex]::Split($raw, '(?m)^---\s*$') | Where-Object { $_.Trim().Length -gt 0 }

$turns = @()
foreach ($block in $turnBlocks) {
    $trimmed = $block.Trim()
    if ($trimmed -match '^\*\*User:\*\*') {
        $turns += @{ role = 'user'; content = $trimmed }
    }
    elseif ($trimmed -match '^\*\*Assistant:\*\*') {
        $turns += @{ role = 'assistant'; content = $trimmed }
    }
    else {
        # Continuation of previous turn or preamble
        if ($turns.Count -gt 0) {
            $turns[-1].content += "`n---`n$trimmed"
        }
    }
}

$totalTurns = $turns.Count

# ============================================================
# STEP 2: Extract elements from turns
# ============================================================

function Extract-Tag {
    param([string]$text, [string]$tagName)
    $pattern = "<$tagName>(.*?)</$tagName>"
    $matches_ = [regex]::Matches($text, $pattern, [System.Text.RegularExpressions.RegexOptions]::Singleline)
    [System.Collections.Generic.List[string]]$results = @()
    foreach ($m in $matches_) {
        $results.Add($m.Groups[1].Value.Trim())
    }
    return @(,$results)
}

function Extract-McpToolCall {
    param([string]$text)
    $pattern = '<use_mcp_tool>\s*<server_name>(.*?)</server_name>\s*<tool_name>(.*?)</tool_name>\s*<arguments>(.*?)</arguments>\s*</use_mcp_tool>'
    $matches_ = [regex]::Matches($text, $pattern, [System.Text.RegularExpressions.RegexOptions]::Singleline)
    [System.Collections.Generic.List[hashtable]]$results = @()
    foreach ($m in $matches_) {
        $results.Add(@{
            server = $m.Groups[1].Value.Trim()
            tool   = $m.Groups[2].Value.Trim()
            args   = $m.Groups[3].Value.Trim()
        })
    }
    return @(,$results)
}

function Extract-McpResult {
    param([string]$text)
    # Pattern: [use_mcp_tool for 'server-name'] Result:
    $pattern = "\[use_mcp_tool for '([^']+)'\] Result:\s*"
    $m = [regex]::Match($text, $pattern)
    if ($m.Success) {
        $afterResult = $text.Substring($m.Index + $m.Length)
        # Strip <environment_details> block
        $afterResult = [regex]::Replace($afterResult, '<environment_details>.*?</environment_details>', '', [System.Text.RegularExpressions.RegexOptions]::Singleline)
        # Strip REMINDERS table
        $afterResult = [regex]::Replace($afterResult, '(?m)^====\s*\nREMINDERS.*', '', [System.Text.RegularExpressions.RegexOptions]::Singleline)
        return @{
            server  = $m.Groups[1].Value.Trim()
            content = $afterResult.Trim()
        }
    }
    return $null
}

function Extract-BuiltinToolCalls {
    param([string]$text)
    [System.Collections.Generic.List[string]]$tools = @()
    if ($text -match '<update_todo_list>') { $tools.Add('update_todo_list') }
    if ($text -match '<read_file>') { $tools.Add('read_file') }
    if ($text -match '<apply_diff>') { $tools.Add('apply_diff') }
    if ($text -match '<write_to_file>') { $tools.Add('write_to_file') }
    if ($text -match '<search_files>') { $tools.Add('search_files') }
    if ($text -match '<execute_command>') { $tools.Add('execute_command') }
    if ($text -match '<insert_content>') { $tools.Add('insert_content') }
    if ($text -match '<search_and_replace>') { $tools.Add('search_and_replace') }
    if ($text -match '<attempt_completion>') { $tools.Add('attempt_completion') }
    if ($text -match '<ask_followup_question>') { $tools.Add('ask_followup_question') }
    if ($text -match '<list_files>') { $tools.Add('list_files') }
    if ($text -match '<list_code_definition_names>') { $tools.Add('list_code_definition_names') }
    return @(,$tools)
}

function Extract-BuiltinToolCallDetails {
    <#
    .SYNOPSIS
        Extracts detailed information about built-in tool calls, including file paths
        and extensions for read_file and search_files calls.
    #>
    param([string]$text)
    [System.Collections.Generic.List[hashtable]]$details = @()

    # read_file: extract file paths from <read_file><args><file><path>...</path></file></args></read_file>
    $readFileBlocks = [regex]::Matches($text, '<read_file>\s*<args>(.*?)</args>\s*</read_file>', [System.Text.RegularExpressions.RegexOptions]::Singleline)
    foreach ($m in $readFileBlocks) {
        $pathMatches = [regex]::Matches($m.Groups[1].Value, '<path>(.*?)</path>')
        $paths = @($pathMatches | ForEach-Object { $_.Groups[1].Value.Trim() })
        $extensions = @($paths | ForEach-Object {
            $ext = [System.IO.Path]::GetExtension($_)
            if ($ext) { $ext.TrimStart('.').ToLower() }
        } | Where-Object { $_ })
        $details.Add(@{
            tool = 'read_file'
            paths = $paths
            extensions = @($extensions | Sort-Object -Unique)
        })
    }

    # search_files: extract path and regex
    $searchFilesBlocks = [regex]::Matches($text, '<search_files>\s*(.*?)\s*</search_files>', [System.Text.RegularExpressions.RegexOptions]::Singleline)
    foreach ($m in $searchFilesBlocks) {
        $pathMatch = [regex]::Match($m.Groups[1].Value, '<path>(.*?)</path>')
        $regexMatch = [regex]::Match($m.Groups[1].Value, '<regex>(.*?)</regex>')
        $searchPath = if ($pathMatch.Success) { $pathMatch.Groups[1].Value.Trim() } else { '' }
        $searchRegex = if ($regexMatch.Success) { $regexMatch.Groups[1].Value.Trim() } else { '' }
        $details.Add(@{
            tool = 'search_files'
            searchPath = $searchPath
            searchRegex = $searchRegex
            paths = @($searchPath)
            extensions = @()
        })
    }

    # list_files and list_code_definition_names (policy violations when search-index available)
    if ($text -match '<list_files>') {
        $details.Add(@{ tool = 'list_files'; paths = @(); extensions = @() })
    }
    if ($text -match '<list_code_definition_names>') {
        $details.Add(@{ tool = 'list_code_definition_names'; paths = @(); extensions = @() })
    }

    # Other built-in tools (simple detection, no path extraction needed)
    $simpleTools = @('update_todo_list', 'apply_diff', 'write_to_file', 'execute_command',
                     'insert_content', 'search_and_replace', 'attempt_completion', 'ask_followup_question')
    foreach ($tool in $simpleTools) {
        $toolPattern = "<$tool>"
        if ($text.Contains($toolPattern)) {
            $details.Add(@{ tool = $tool; paths = @(); extensions = @() })
        }
    }

    return @(,$details)
}

function Extract-IndexedExtensions {
    <#
    .SYNOPSIS
        Extracts the list of indexed file extensions from policyReminder in MCP responses.
        Pattern: "Indexed extensions: ts" or "Indexed extensions: xml,config,txt,ts"
    #>
    param([string]$resultText)
    $m = [regex]::Match($resultText, 'Indexed extensions:\s*([a-zA-Z0-9,]+)')
    if ($m.Success) {
        return @($m.Groups[1].Value -split ',' | ForEach-Object { $_.Trim().ToLower() } | Where-Object { $_ })
    }
    return @()
}

function Summarize-Text {
    param([string]$text, [int]$maxLen = 200)
    if ($text.Length -le $maxLen) { return $text }
    return $text.Substring(0, $maxLen) + "..."
}

function Normalize-Params {
    param([string]$argsJson)
    try {
        $obj = $argsJson | ConvertFrom-Json -ErrorAction Stop
        $parts = @()
        foreach ($prop in ($obj.PSObject.Properties | Sort-Object Name)) {
            $val = $prop.Value
            if ($val -is [string] -and $val.Length -gt 80) {
                $val = $val.Substring(0, 77) + "..."
            }
            $parts += "$($prop.Name)=$val"
        }
        return ($parts -join ', ')
    }
    catch {
        return (Summarize-Text $argsJson 120)
    }
}

function Classify-ResponseStatus {
    param([string]$resultText)
    if ([string]::IsNullOrWhiteSpace($resultText)) {
        return @{ status = 'empty'; reason = 'empty response' }
    }
    # Check for actual errors - not just "error" substring in content
    # Tool errors typically start with error message or have specific error structure
    if ($resultText -match '^\s*The tool execution failed' -or $resultText -match '^\s*Error:' -or $resultText -match '"error"\s*:\s*"[^"]*"' -or $resultText -match 'Missing value for required parameter') {
        # But not if it also has definitions/files (false positive - "error" appears in code content)
        if ($resultText -notmatch '"definitions"\s*:\s*\[' -and $resultText -notmatch '"files"\s*:\s*\[') {
            return @{ status = 'error'; reason = (Summarize-Text $resultText 100) }
        }
    }
    if ($resultText -match '"responseTruncated"\s*:\s*true' -or $resultText -match '"truncated"' -or $resultText -match 'Response truncated') {
        $reason = ''
        $m = [regex]::Match($resultText, '"truncationReason"\s*:\s*"([^"]+)"')
        if ($m.Success) { $reason = $m.Groups[1].Value }
        elseif ($resultText -match 'Response truncated') { $reason = 'response truncated' }
        return @{ status = 'partial'; reason = $reason }
    }
    if ($resultText -match '"totalResults"\s*:\s*0[^0-9]') {
        return @{ status = 'empty'; reason = 'totalResults: 0' }
    }
    return @{ status = 'success'; reason = '' }
}

function Extract-ResponseSummary {
    param([string]$resultText)
    try {
        $json = $resultText | ConvertFrom-Json -ErrorAction Stop
        if ($json.summary) {
            $parts = @()
            if ($null -ne $json.summary.totalResults) { $parts += "totalResults: $($json.summary.totalResults)" }
            if ($null -ne $json.summary.returned) { $parts += "returned: $($json.summary.returned)" }
            if ($null -ne $json.summary.searchTimeMs) { $parts += "searchTimeMs: $([math]::Round($json.summary.searchTimeMs, 1))ms" }
            if ($null -ne $json.summary.responseBytes) { $parts += "responseBytes: $($json.summary.responseBytes)" }
            if ($json.summary.hint) { $parts += "hint: $(Summarize-Text $json.summary.hint 120)" }
            if ($json.summary.responseTruncated) {
                $parts += "TRUNCATED"
                if ($json.summary.truncationReason) { $parts += "reason: $($json.summary.truncationReason)" }
            }
            if ($null -ne $json.summary.estimatedTokens) { $parts += "tokens: ~$($json.summary.estimatedTokens)" }
            if ($parts.Count -gt 0) { return $parts -join ', ' }
        }
        if ($json.definitions) {
            $count = if ($json.definitions -is [array]) { $json.definitions.Count } else { 0 }
            return "definitions: $count items"
        }
        if ($json.files) {
            $count = if ($json.files -is [array]) { $json.files.Count } else { 0 }
            return "files: $count items"
        }
    }
    catch {}
    return Summarize-Text $resultText 150
}

function Get-ParamDiff {
    param([string]$prevArgs, [string]$currArgs)
    try {
        $prev = $prevArgs | ConvertFrom-Json -ErrorAction Stop
        $curr = $currArgs | ConvertFrom-Json -ErrorAction Stop
        $diffs = @()
        $allKeys = @($prev.PSObject.Properties.Name) + @($curr.PSObject.Properties.Name) | Sort-Object -Unique
        foreach ($key in $allKeys) {
            $pVal = if ($prev.PSObject.Properties[$key]) { "$($prev.$key)" } else { '<absent>' }
            $cVal = if ($curr.PSObject.Properties[$key]) { "$($curr.$key)" } else { '<absent>' }
            if ($pVal -ne $cVal) {
                $diffs += "${key}: $(Summarize-Text $pVal 50) -> $(Summarize-Text $cVal 50)"
            }
        }
        if ($diffs.Count -eq 0) { return 'identical' }
        return $diffs -join '; '
    }
    catch { return '' }
}

function Is-TargetRefinement {
    <#
    .SYNOPSIS
        Determines whether two consecutive calls to the same tool represent a true
        scope refinement (same target entity, changed scope parameters) vs sequential
        exploration (different target entity).
    .DESCRIPTION
        Compares "target identity" parameters between two calls:
        - search_definitions: name, parent, file
        - search_callers: method, class
        - search_grep: terms
        - search_fast: pattern
        - search_edit: path

        Returns $true if there is meaningful overlap in target identity, indicating
        the model is refining the same query. Returns $false if targets are completely
        different, indicating sequential exploration of different entities.
    #>
    param([string]$prevArgs, [string]$currArgs, [string]$toolName)
    try {
        $prev = $prevArgs | ConvertFrom-Json -ErrorAction Stop
        $curr = $currArgs | ConvertFrom-Json -ErrorAction Stop

        # Define target keys per tool
        $targetKeys = switch ($toolName) {
            'search_definitions' { @('name', 'parent', 'file') }
            'search_callers'     { @('method', 'class') }
            'search_grep'        { @('terms') }
            'search_fast'        { @('pattern') }
            'search_edit'        { @('path') }
            default              { @('name', 'file', 'method', 'path', 'terms', 'pattern') }
        }

        # Collect target values from each call
        $prevTargets = @{}
        $currTargets = @{}
        foreach ($key in $targetKeys) {
            $pVal = if ($prev.PSObject.Properties[$key]) { "$($prev.$key)".Trim() } else { '' }
            $cVal = if ($curr.PSObject.Properties[$key]) { "$($curr.$key)".Trim() } else { '' }
            if ($pVal) { $prevTargets[$key] = $pVal }
            if ($cVal) { $currTargets[$key] = $cVal }
        }

        # If neither call has any target params, treat as refinement (scope-only change)
        if ($prevTargets.Count -eq 0 -and $currTargets.Count -eq 0) { return $true }

        # Rule 1: Direct key match — same key, same value (or one is substring of other)
        foreach ($key in $targetKeys) {
            $pVal = if ($prevTargets.ContainsKey($key)) { $prevTargets[$key] } else { '' }
            $cVal = if ($currTargets.ContainsKey($key)) { $currTargets[$key] } else { '' }
            if ($pVal -and $cVal) {
                # Exact match
                if ($pVal -eq $cVal) { return $true }
                # File path: one is subdirectory of the other (drilling down)
                if ($key -eq 'file') {
                    if ($cVal.Contains($pVal) -or $pVal.Contains($cVal)) { return $true }
                }
                # Comma-separated names: check overlap (e.g., "A,B,C" vs "B")
                $pNames = $pVal -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
                $cNames = $cVal -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
                $overlap = $pNames | Where-Object { $cNames -contains $_ }
                if ($overlap) { return $true }
            }
        }

        # Rule 2: Cross-key match — name↔parent (drilling from class to its methods)
        if ($toolName -eq 'search_definitions') {
            $pName   = if ($prevTargets.ContainsKey('name'))   { $prevTargets['name'] }   else { '' }
            $pParent = if ($prevTargets.ContainsKey('parent')) { $prevTargets['parent'] } else { '' }
            $cName   = if ($currTargets.ContainsKey('name'))   { $currTargets['name'] }   else { '' }
            $cParent = if ($currTargets.ContainsKey('parent')) { $currTargets['parent'] } else { '' }

            # prev.name == curr.parent (looked up class, now exploring its methods)
            if ($pName -and $cParent) {
                $pNames = $pName -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
                $cParents = $cParent -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
                $overlap = $pNames | Where-Object { $cParents -contains $_ }
                if ($overlap) { return $true }
            }
            # prev.parent == curr.name (reverse)
            if ($pParent -and $cName) {
                $pParents = $pParent -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
                $cNames = $cName -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
                $overlap = $pParents | Where-Object { $cNames -contains $_ }
                if ($overlap) { return $true }
            }
        }

        # No target overlap found — this is sequential exploration
        return $false
    }
    catch {
        # If parsing fails, assume refinement (conservative)
        return $true
    }
}

function Analyze-TruncationCause {
    param([hashtable]$episode)
    $causes = @()

    # Check response data for response-size-limit indicators
    $resultText = $episode._result_raw
    if ($resultText -match 'capped lines' -or $resultText -match 'removed lineContent' -or $resultText -match 'lineContentOmitted') {
        $causes += 'response_size_limit'
    }
    # Detect when returned < maxResults despite totalResults > returned (byte budget exceeded)
    $returnedMatch = [regex]::Match($resultText, '"returned"\s*:\s*(\d+)')
    $totalMatch = [regex]::Match($resultText, '"totalResults"\s*:\s*(\d+)')
    if ($returnedMatch.Success -and $totalMatch.Success) {
        $returned = [int]$returnedMatch.Groups[1].Value
        $total = [int]$totalMatch.Groups[1].Value
        try {
            $args_ = $episode._call_args | ConvertFrom-Json -ErrorAction Stop
            $maxR = if ($args_.maxResults) { [int]$args_.maxResults } else { 100 }
            if ($returned -lt $maxR -and $total -gt $returned) {
                # Truncated below maxResults — byte budget was the limit
                if ($causes -notcontains 'response_size_limit') {
                    $causes += 'response_size_limit'
                }
            }
        } catch {}
    }

    try {
        $args_ = $episode._call_args | ConvertFrom-Json -ErrorAction Stop
        # Wide file scope (no specific file or very broad path)
        if ($args_.file) {
            $fileVal = "$($args_.file)"
            $commaCount = ($fileVal.ToCharArray() | Where-Object { $_ -eq ',' }).Count
            if ($commaCount -ge 3) { $causes += "wide_file_scope ($($commaCount + 1) paths)" }
            if ($fileVal -notmatch '/' -or $fileVal.Length -lt 20) { $causes += "broad_directory" }
        }
        # Large maxResults
        if ($args_.maxResults -and [int]$args_.maxResults -gt 50) {
            $causes += "high_maxResults ($($args_.maxResults))"
        }
        # NOTE: no_kind_filter and no_name_filter removed — they are noise for exploration sessions
        # where broad queries without kind/name are normal and expected.
    }
    catch {}

    # Tool-specific truncation markers
    $toolName = $episode.tool
    if ($toolName -eq 'search_callers') {
        # Callers: body truncation, maxTotalNodes, maxCallersPerLevel
        if ($resultText -match '"bodyTruncated"\s*:\s*true' -or $resultText -match 'bodyOmitted') {
            $causes += 'body_truncation'
        }
        if ($resultText -match 'maxTotalNodes' -or $resultText -match 'truncated.*nodes') {
            $causes += 'max_total_nodes'
        }
        if ($resultText -match '"callTree"\s*:\s*\[\]' -and $resultText -match '"hint"') {
            $causes += 'no_callers_found (DI/interface mismatch)'
        }
        # Callers includeBody responses are typically small but truncated by body budget
        if ($resultText -match '"body"' -and $episode.response_size_bytes -gt 3000) {
            if ($causes.Count -eq 0) { $causes += 'body_budget_limit (inferred)' }
        }
    }
    elseif ($toolName -eq 'search_definitions') {
        # Definitions: totalResults > returned count (definitions were truncated)
        if ($returnedMatch.Success -and $totalMatch.Success) {
            $returned = [int]$returnedMatch.Groups[1].Value
            $total = [int]$totalMatch.Groups[1].Value
            if ($total -gt $returned) {
                if ($causes -notcontains 'response_size_limit') {
                    $causes += 'definitions_truncated'
                }
            }
        }
    }

    # Fallback: infer response_size_limit from response size when no specific cause found
    # Skip if we already have a specific cause (definitions_truncated, body_truncation, etc.)
    $specificCauses = @('response_size_limit', 'definitions_truncated', 'body_truncation', 'max_total_nodes')
    $hasSpecificCause = $false
    foreach ($sc in $specificCauses) {
        if ($causes -contains $sc) { $hasSpecificCause = $true; break }
    }
    if ($episode.response_size_bytes -gt 15000 -and -not $hasSpecificCause) {
        $causes += 'response_size_limit (inferred from response size)'
    }

    if ($causes.Count -eq 0) { return 'unknown' }
    return $causes -join ', '
}

function Has-Hint {
    param([string]$resultText)
    return ($resultText -match '"hint"\s*:' -or $resultText -match '"nextStepHint"\s*:')
}

function Has-CorrectionHint {
    param([string]$resultText)
    # Only truncation/correction hints, not informational nextStepHint
    return ($resultText -match '"hint"\s*:\s*"[^"]*(?:truncat|narrow|reduce|filter|scope)[^"]*"')
}

function Extract-Hints {
    param([string]$resultText)
    [System.Collections.Generic.List[hashtable]]$hints = @()
    $m = [regex]::Match($resultText, '"hint"\s*:\s*"([^"]+)"')
    if ($m.Success) {
        $hintText = $m.Groups[1].Value
        $isCorrective = $hintText -match '(?i)truncat|narrow|reduce|filter|scope|too many'
        $hints.Add(@{ text = $hintText; type = if ($isCorrective) { 'correction' } else { 'informational' } })
    }
    $m = [regex]::Match($resultText, '"nextStepHint"\s*:\s*"([^"]+)"')
    if ($m.Success) {
        $hints.Add(@{ text = $m.Groups[1].Value; type = 'informational' })
    }
    return @(,$hints)
}

# ============================================================
# STEP 3: Build episodes
# ============================================================

[System.Collections.Generic.List[hashtable]]$episodes = @()
$builtinCallCount = 0
$builtinCallDetails = @{}
[System.Collections.Generic.List[hashtable]]$builtinCallDetailsList = @()
[System.Collections.Generic.List[string]]$indexedExtensions = @()
$mcpAvailable = $true  # assume available until first error
$mcpBecameAvailable = $false  # tracks if MCP recovered after errors
[System.Collections.Generic.List[hashtable]]$policyViolations = @()
[System.Collections.Generic.List[hashtable]]$autoCorrections = @()
$totalEstimatedTokens = 0
$sessionMode = ''
$taskDescription = ''
$completionText = ''

for ($i = 0; $i -lt $turns.Count; $i++) {
    $turn = $turns[$i]

    # Extract task description from first user turn
    if ($turn.role -eq 'user' -and -not $taskDescription) {
        $taskTags = Extract-Tag $turn.content 'task'
        if ($taskTags.Count -gt 0) {
            $taskDescription = Summarize-Text $taskTags[0] 300
        }
    }

    # Extract session mode from environment_details (improvement 6)
    if ($turn.role -eq 'user' -and -not $sessionMode) {
        $slugMatch = [regex]::Match($turn.content, '<slug>([^<]+)</slug>')
        if ($slugMatch.Success) {
            $sessionMode = $slugMatch.Groups[1].Value.Trim()
        }
    }

    if ($turn.role -ne 'assistant') { continue }

    # Count built-in tools (simple counts for backward compat)
    $builtins = Extract-BuiltinToolCalls $turn.content
    foreach ($bt in $builtins) {
        if ($bt -eq 'attempt_completion') {
            $compTags = Extract-Tag $turn.content 'attempt_completion'
            if ($compTags.Count -gt 0) { $completionText = $compTags[0] }
            continue
        }
        $builtinCallCount++
        if (-not $builtinCallDetails.ContainsKey($bt)) { $builtinCallDetails[$bt] = 0 }
        $builtinCallDetails[$bt]++
    }

    # Detailed built-in tool extraction (paths, extensions, policy violations)
    $builtinDetails = Extract-BuiltinToolCallDetails $turn.content
    foreach ($bd in $builtinDetails) {
        if ($bd.tool -eq 'attempt_completion') { continue }
        $builtinCallDetailsList.Add($bd)

        # Check for policy violations: read_file for indexed file types
        if ($bd.tool -eq 'read_file' -and $indexedExtensions.Count -gt 0 -and $bd.extensions.Count -gt 0) {
            foreach ($ext in $bd.extensions) {
                if ($indexedExtensions -contains $ext) {
                    $policyViolations.Add(@{
                        turn = $i
                        tool = 'read_file'
                        paths = $bd.paths
                        extension = $ext
                        mcp_available = $mcpAvailable
                        suggested_alternative = 'search_definitions includeBody=true maxBodyLines=0'
                        reason = "read_file used for .$ext file (indexed extension) — should use search_definitions"
                    })
                    break  # one violation per call
                }
            }
        }

        # Check for policy violations: search_files when search_grep is available
        if ($bd.tool -eq 'search_files' -and $indexedExtensions.Count -gt 0) {
            $policyViolations.Add(@{
                turn = $i
                tool = 'search_files'
                paths = $bd.paths
                extension = ''
                mcp_available = $mcpAvailable
                suggested_alternative = 'search_grep'
                reason = 'search_files used instead of search_grep (search-index MCP available)'
            })
        }

        # Check for policy violations: list_files when search_fast is available
        if ($bd.tool -eq 'list_files' -and $indexedExtensions.Count -gt 0) {
            $policyViolations.Add(@{
                turn = $i
                tool = 'list_files'
                paths = $bd.paths
                extension = ''
                mcp_available = $mcpAvailable
                suggested_alternative = 'search_fast'
                reason = 'list_files used instead of search_fast (search-index MCP available)'
            })
        }

        # Check for policy violations: list_code_definition_names when search_definitions is available
        if ($bd.tool -eq 'list_code_definition_names' -and $indexedExtensions.Count -gt 0) {
            $policyViolations.Add(@{
                turn = $i
                tool = 'list_code_definition_names'
                paths = $bd.paths
                extension = ''
                mcp_available = $mcpAvailable
                suggested_alternative = 'search_definitions'
                reason = 'list_code_definition_names used instead of search_definitions (search-index MCP available)'
            })
        }
    }

    # Extract MCP tool calls
    $mcpCalls = Extract-McpToolCall $turn.content
    if ($mcpCalls.Count -eq 0) { continue }

    # Extract thinking before
    $thinkings = Extract-Tag $turn.content 'thinking'
    $thinkingBefore = if ($thinkings.Count -gt 0) { Summarize-Text $thinkings[-1] 300 } else { '' }

    foreach ($call in $mcpCalls) {
        # Find the result in the next user turn
        $resultContent = ''
        $resultServer = ''
        if ($i + 1 -lt $turns.Count -and $turns[$i + 1].role -eq 'user') {
            $mcpResult = Extract-McpResult $turns[$i + 1].content
            if ($mcpResult) {
                $resultContent = $mcpResult.content
                $resultServer = $mcpResult.server
            }
        }

        # Determine reaction_after: next assistant turn's thinking or content
        $reactionAfter = @{ type = 'none'; content = '' }
        if ($i + 2 -lt $turns.Count -and $turns[$i + 2].role -eq 'assistant') {
            $nextAssistant = $turns[$i + 2].content
            $nextThinkings = Extract-Tag $nextAssistant 'thinking'
            $nextMcpCalls = Extract-McpToolCall $nextAssistant

            if ($nextAssistant -match '<attempt_completion>') {
                $reactionAfter = @{ type = 'completion'; content = '' }
            }
            elseif ($nextThinkings.Count -gt 0) {
                $reactionAfter = @{ type = 'thinking'; content = Summarize-Text $nextThinkings[0] 300 }
            }
            elseif ($nextMcpCalls.Count -gt 0) {
                $reactionAfter = @{ type = 'immediate_next_call'; content = "$($nextMcpCalls[0].server)/$($nextMcpCalls[0].tool)" }
            }
            else {
                # Text follow-up (strip noise)
                $cleanText = $nextAssistant -replace '^\*\*Assistant:\*\*\s*', ''
                $cleanText = [regex]::Replace($cleanText, '<environment_details>.*?</environment_details>', '', [System.Text.RegularExpressions.RegexOptions]::Singleline)
                if ($cleanText.Trim().Length -gt 10) {
                    $reactionAfter = @{ type = 'text'; content = Summarize-Text $cleanText.Trim() 300 }
                }
            }
        }

        $status = Classify-ResponseStatus $resultContent
        $responseSummary = Extract-ResponseSummary $resultContent
        $responseSize = [System.Text.Encoding]::UTF8.GetByteCount($resultContent)

        # Extract estimatedTokens from response (improvement 2), with fallback to bytes/4
        $estimatedTokens = 0
        $tokensMatch = [regex]::Match($resultContent, '"estimatedTokens"\s*:\s*(\d+)')
        if ($tokensMatch.Success) {
            $estimatedTokens = [int]$tokensMatch.Groups[1].Value
        }
        elseif ($responseSize -gt 0) {
            # Fallback: estimate tokens from response size (~4 bytes per token)
            $estimatedTokens = [math]::Round($responseSize / 4, 0)
        }
        $totalEstimatedTokens += $estimatedTokens

        # Extract autoCorrection from response (improvement 1)
        $autoCorrectionInfo = $null
        $acTypeMatch = [regex]::Match($resultContent, '"autoCorrection"\s*:\s*\{.*?"type"\s*:\s*"([^"]+)"')
        if ($acTypeMatch.Success) {
            $acType = $acTypeMatch.Groups[1].Value
            $acReasonMatch = [regex]::Match($resultContent, '"autoCorrection"\s*:\s*\{.*?"reason"\s*:\s*"([^"]+)"')
            $acReason = if ($acReasonMatch.Success) { $acReasonMatch.Groups[1].Value } else { 'unknown' }
            $autoCorrectionInfo = @{
                type   = $acType
                reason = $acReason
            }
            $autoCorrections.Add(@{
                episode = $episodes.Count + 1
                tool    = $call.tool
                type    = $acType
                reason  = $acReason
            })
        }

        # Count bodyOmitted entries (improvement 4)
        $bodyOmittedCount = ([regex]::Matches($resultContent, '"bodyOmitted"\s*:')).Count

        # Extract termBreakdown (improvement 5)
        $termBreakdown = $null
        $tbMatch = [regex]::Match($resultContent, '"termBreakdown"\s*:\s*(\{[^}]+\})')
        if ($tbMatch.Success) {
            try {
                $termBreakdown = $tbMatch.Groups[1].Value | ConvertFrom-Json -ErrorAction Stop
            } catch { $termBreakdown = $null }
        }

        # Track MCP availability (for policy violation context)
        if ($call.server -eq 'search-index') {
            if ($status.status -eq 'error') {
                $mcpAvailable = $false
            }
            else {
                if (-not $mcpAvailable) { $mcpBecameAvailable = $true }
                $mcpAvailable = $true
            }

            # Extract indexed extensions from policyReminder (once)
            if ($indexedExtensions.Count -eq 0 -and $status.status -ne 'error') {
                $extractedExts = Extract-IndexedExtensions $resultContent
                foreach ($ext in $extractedExts) {
                    $indexedExtensions.Add($ext)
                }
            }
        }

        # Compute param_diff from previous episode (if same tool)
        $paramDiff = ''
        if ($episodes.Count -gt 0) {
            $prevEp = $episodes[$episodes.Count - 1]
            if ($prevEp.tool -eq $call.tool -and $prevEp.server -eq $call.server) {
                $paramDiff = Get-ParamDiff $prevEp._call_args $call.args
            }
        }

        $newEp = @{
            index               = $episodes.Count + 1
            server              = $call.server
            tool                = $call.tool
            params_summary      = Normalize-Params $call.args
            response_status     = $status.status
            response_size_bytes = $responseSize
            response_summary    = $responseSummary
            partial_reason      = $status.reason
            thinking_before     = $thinkingBefore
            reaction_after      = $reactionAfter
            result_used         = $false
            tags                = [System.Collections.Generic.List[string]]::new()
            param_diff          = $paramDiff
            truncation_cause    = ''
            estimated_tokens    = $estimatedTokens
            auto_correction     = $autoCorrectionInfo
            body_omitted_count  = $bodyOmittedCount
            term_breakdown      = $termBreakdown
            _result_raw         = $resultContent
            _call_args          = $call.args
            _hints              = @(Extract-Hints $resultContent)
        }

        if ($status.status -eq 'partial') {
            $newEp.truncation_cause = Analyze-TruncationCause $newEp
        }

        $episodes.Add($newEp)
    }
}

# ============================================================
# STEP 4: Compute tags and result_used
# ============================================================

for ($e = 0; $e -lt $episodes.Count; $e++) {
    $ep = $episodes[$e]

    # first_call / final_call
    $sameToolBefore = @($episodes[0..([math]::Max(0, $e - 1))] | Where-Object { $_.tool -eq $ep.tool -and $_ -ne $ep }).Count
    $sameToolAfter = @($episodes[([math]::Min($e + 1, $episodes.Count - 1))..($episodes.Count - 1)] | Where-Object { $_.tool -eq $ep.tool -and $_ -ne $ep }).Count

    if ($sameToolBefore -eq 0) { $ep.tags.Add('first_call') }
    if ($sameToolAfter -eq 0) { $ep.tags.Add('final_call') }

    # truncated_response
    if ($ep.response_status -eq 'partial') { $ep.tags.Add('truncated_response') }

    # heavy_response: large non-truncated response (improvement 3)
    if ($ep.response_status -eq 'success' -and $ep.response_size_bytes -gt 20000) {
        $ep.tags.Add('heavy_response')
    }

    # auto_corrected: server auto-corrected the query (improvement 1)
    if ($ep.auto_correction) {
        $ep.tags.Add('auto_corrected')
    }

    # progressive_refinement vs sequential_exploration vs retry
    if ($e -gt 0) {
        $prev = $episodes[$e - 1]
        if ($prev.tool -eq $ep.tool -and $prev.server -eq $ep.server) {
            # Compare args: if identical -> retry
            if ($prev._call_args -eq $ep._call_args) {
                $ep.tags.Add('retry')
            }
            else {
                # Check if target entity is the same (refinement) or different (exploration)
                $isRefinement = Is-TargetRefinement $prev._call_args $ep._call_args $ep.tool
                if ($isRefinement) {
                    $ep.tags.Add('progressive_refinement')
                }
                else {
                    $ep.tags.Add('sequential_exploration')
                }
            }
        }
    }

    # result_used: check if reaction_after references data from the result
    $reaction = $ep.reaction_after
    if ($reaction.type -eq 'thinking' -and $reaction.content.Length -gt 0) {
        $ep.result_used = $true
        # Check for strategy_change
        if ($reaction.content -match 'instead|should|better|different|narrower|broader|change|too broad|too narrow|truncat') {
            $ep.tags.Add('strategy_change')
        }
    }
    elseif ($reaction.type -eq 'completion') {
        $ep.result_used = $true
    }
    elseif ($reaction.type -eq 'text' -and $reaction.content.Length -gt 20) {
        $ep.result_used = $true
    }
    elseif ($reaction.type -eq 'immediate_next_call') {
        # If next call to same tool with refined params, result was used
        if ($e + 1 -lt $episodes.Count -and $episodes[$e + 1].tags -contains 'progressive_refinement') {
            $ep.result_used = $true
        }
        else {
            # Heuristic: if there's thinking_before in next episode that references this tool
            $ep.result_used = $true  # optimistic default
        }
    }

    # data_quality_complaint: model explicitly flags a data quality issue in thinking
    if ($ep.thinking_before -match '(?i)(showing 0|always 0|fileCount.*0|returns? 0|doesn.t track|doesn.t work|doesn.t count|not working|broken|useless|misleading|no data|empty result|all zero)') {
        $ep.tags.Add('data_quality_complaint')
    }

    # kind_mismatch: detect when term_breakdown shows 0 for some names due to wrong kind filter,
    # and the next episode changes/removes kind and finds those names
    if ($ep.tool -eq 'search_definitions' -and $ep.term_breakdown -and $e + 1 -lt $episodes.Count) {
        $nextEp = $episodes[$e + 1]
        # Check refinement inline (nextEp tags may not be computed yet in this forward pass)
        $isNextRefinement = ($nextEp.tool -eq 'search_definitions' -and $nextEp.server -eq $ep.server -and
            $nextEp._call_args -ne $ep._call_args -and (Is-TargetRefinement $ep._call_args $nextEp._call_args 'search_definitions'))
        if ($nextEp.tool -eq 'search_definitions' -and $isNextRefinement) {
            try {
                $currParams = $ep._call_args | ConvertFrom-Json -ErrorAction Stop
                $nextParams = $nextEp._call_args | ConvertFrom-Json -ErrorAction Stop
                $currKind = if ($currParams.PSObject.Properties['kind']) { "$($currParams.kind)" } else { '' }
                $nextKind = if ($nextParams.PSObject.Properties['kind']) { "$($nextParams.kind)" } else { '' }
                # Only if kind changed between episodes
                if ($currKind -and $currKind -ne $nextKind) {
                    # Check if any names had 0 results in term_breakdown
                    $zeroNames = @()
                    foreach ($prop in $ep.term_breakdown.PSObject.Properties) {
                        if ([int]$prop.Value -eq 0) { $zeroNames += $prop.Name }
                    }
                    if ($zeroNames.Count -gt 0) {
                        # Check if next episode's term_breakdown found some of those names
                        $recoveredNames = @()
                        if ($nextEp.term_breakdown) {
                            foreach ($prop in $nextEp.term_breakdown.PSObject.Properties) {
                                if ([int]$prop.Value -gt 0 -and $zeroNames -contains $prop.Name) {
                                    $recoveredNames += $prop.Name
                                }
                            }
                        }
                        # Tag as kind_mismatch if names had 0 results (even without recovery proof)
                        $ep.tags.Add('kind_mismatch')
                        $ep['kind_mismatch_details'] = @{
                            requested_kind = $currKind
                            next_kind = if ($nextKind) { $nextKind } else { '<removed>' }
                            zero_names = $zeroNames
                            recovered_names = $recoveredNames
                        }
                    }
                }
            } catch {}
        }
    }

    # redundant
    if (-not $ep.result_used -and $ep.response_status -ne 'error') {
        $ep.tags.Add('redundant')
    }

    # hint_followed / hint_ignored
    # Only correction hints count for ignored metric; informational nextStepHints are skipped
    $correctionHints = @($ep._hints | Where-Object { $_ -is [hashtable] -and $_['type'] -eq 'correction' })
    $infoHints = @($ep._hints | Where-Object { $_ -is [hashtable] -and $_['type'] -eq 'informational' })

    if ($correctionHints.Count -gt 0) {
        $hintFollowed = $false
        # Check next 2 MCP episodes (model may do built-in calls between MCP calls)
        $lookAhead = [math]::Min($e + 2, $episodes.Count - 1)
        for ($la = $e + 1; $la -le $lookAhead; $la++) {
            $nextEp = $episodes[$la]
            foreach ($hint in $correctionHints) {
                $hintText = $hint.text
                if ($hintText -match 'search_grep' -and $nextEp.tool -eq 'search_grep') { $hintFollowed = $true }
                elseif ($hintText -match 'search_definitions' -and $nextEp.tool -eq 'search_definitions') { $hintFollowed = $true }
                elseif ($hintText -match 'search_callers' -and $nextEp.tool -eq 'search_callers') { $hintFollowed = $true }
                elseif ($hintText -match 'search_fast' -and $nextEp.tool -eq 'search_fast') { $hintFollowed = $true }
                # Also count as followed if same tool with different params (progressive refinement)
                elseif ($nextEp.tool -eq $ep.tool -and $nextEp.server -eq $ep.server -and $nextEp._call_args -ne $ep._call_args) { $hintFollowed = $true }
            }
        }
        if ($hintFollowed) { $ep.tags.Add('hint_followed') }
        else { $ep.tags.Add('hint_ignored') }
    }
    elseif ($infoHints.Count -gt 0) {
        # Informational hints: check if followed but don't flag as ignored
        $hintFollowed = $false
        if ($e + 1 -lt $episodes.Count) {
            $nextEp = $episodes[$e + 1]
            foreach ($hint in $infoHints) {
                $hintText = $hint.text
                if ($hintText -match 'search_grep' -and $nextEp.tool -eq 'search_grep') { $hintFollowed = $true }
                elseif ($hintText -match 'search_definitions' -and $nextEp.tool -eq 'search_definitions') { $hintFollowed = $true }
                elseif ($hintText -match 'search_callers' -and $nextEp.tool -eq 'search_callers') { $hintFollowed = $true }
                elseif ($hintText -match 'search_fast' -and $nextEp.tool -eq 'search_fast') { $hintFollowed = $true }
            }
        }
        if ($hintFollowed) { $ep.tags.Add('hint_followed') }
        # Don't add 'hint_ignored' for informational hints — it's not a problem
    }
}

# ============================================================
# STEP 5: Phase model
# ============================================================

$firstMcpTurn = -1
$lastMcpTurn = -1
for ($i = 0; $i -lt $turns.Count; $i++) {
    if ($turns[$i].role -eq 'assistant') {
        $calls = Extract-McpToolCall $turns[$i].content
        if ($calls.Count -gt 0) {
            if ($firstMcpTurn -eq -1) { $firstMcpTurn = $i }
            $lastMcpTurn = $i
        }
    }
}

$setupTurns = if ($firstMcpTurn -gt 0) { $firstMcpTurn } else { 0 }
$synthesisTurns = if ($lastMcpTurn -ge 0) { $totalTurns - $lastMcpTurn - 1 } else { 0 }
$explorationTurns = $totalTurns - $setupTurns - $synthesisTurns

$hasCompletion = $completionText.Length -gt 0
$hasSelfAnalysis = $false
$selfAnalysis = @{}

if ($hasCompletion) {
    # Detect self-analysis sections
    if ($completionText -match '(?i)(suboptimal|session analysis|tool improvement|stats)') {
        $hasSelfAnalysis = $true

        # Extract suboptimal count
        $suboptimalMatch = [regex]::Match($completionText, '(?i)suboptimal.*?(\d+)')
        if ($suboptimalMatch.Success) {
            $selfAnalysis['suboptimal_queries'] = [int]$suboptimalMatch.Groups[1].Value
        }

        # Extract percentage - prefer "suboptimal...N%" pattern
        $pctMatch = [regex]::Match($completionText, '(?i)suboptimal.*?(\d+)\s*%')
        if (-not $pctMatch.Success) {
            $pctMatch = [regex]::Match($completionText, '(\d+)\s*%')
        }
        if ($pctMatch.Success) {
            $selfAnalysis['suboptimal_pct'] = [int]$pctMatch.Groups[1].Value
        }

        # Extract improvement ideas - scoped to "Tool Improvement" section only
        $ideasSection = [regex]::Match($completionText, '(?ims)(?:tool\s+improvement|improvement\s+ideas)[^\n]*\n(.*?)(?=\n##|\n###|\n\*\*[A-Z]|\nStats|\z)')
        if ($ideasSection.Success) {
            $ideasText = $ideasSection.Groups[1].Value
            $ideasMatch = [regex]::Matches($ideasText, '(?m)^-\s+(.+)$')
            if ($ideasMatch.Count -gt 0) {
                $selfAnalysis['improvement_ideas'] = @($ideasMatch | ForEach-Object { $_.Groups[1].Value.Trim() })
            }
        }

        # Fallback: if hasSelfAnalysis but no specific patterns matched, extract raw text
        if ($selfAnalysis.Count -eq 0) {
            $selfAnalysis['raw_text'] = (Summarize-Text $completionText 500)
        }
    }
}

$phases = @{
    setup       = @{ turns = $setupTurns; description = (Summarize-Text $taskDescription 200) }
    exploration = @{ turns = $explorationTurns; mcp_calls = $episodes.Count }
    synthesis   = @{ turns = $synthesisTurns; has_completion = $hasCompletion; has_self_analysis = $hasSelfAnalysis }
}

# ============================================================
# STEP 6: Tool quality scorecard
# ============================================================

$toolScorecard = @{}
$toolGroups = $episodes | Group-Object -Property tool

foreach ($group in $toolGroups) {
    $toolName = $group.Name
    $toolEps = $group.Group

    $usedCount = @($toolEps | Where-Object { $_.result_used }).Count
    $truncCount = @($toolEps | Where-Object { $_.response_status -eq 'partial' }).Count
    $emptyCount = @($toolEps | Where-Object { $_.response_status -eq 'empty' }).Count
    $errorCount = @($toolEps | Where-Object { $_.response_status -eq 'error' }).Count
    $refinementCount = @($toolEps | Where-Object { $_.tags -contains 'progressive_refinement' }).Count
    $seqExplCount = @($toolEps | Where-Object { $_.tags -contains 'sequential_exploration' }).Count
    $strategyCount = @($toolEps | Where-Object { $_.tags -contains 'strategy_change' }).Count
    $hintsFollowed = @($toolEps | Where-Object { $_.tags -contains 'hint_followed' }).Count
    $hintsIgnored = @($toolEps | Where-Object { $_.tags -contains 'hint_ignored' }).Count
    $avgSize = [math]::Round(($toolEps | Measure-Object -Property response_size_bytes -Average).Average, 0)

    # first_useful_call: ordinal within this tool's calls
    $firstUseful = 0
    for ($j = 0; $j -lt $toolEps.Count; $j++) {
        if ($toolEps[$j].result_used) { $firstUseful = $j + 1; break }
    }

    $autoCorrectedCount = @($toolEps | Where-Object { $_.tags -contains 'auto_corrected' }).Count
    $heavyCount = @($toolEps | Where-Object { $_.tags -contains 'heavy_response' }).Count
    $totalTokens = ($toolEps | Measure-Object -Property estimated_tokens -Sum).Sum
    $totalBodyOmitted = ($toolEps | Measure-Object -Property body_omitted_count -Sum).Sum

    $toolScorecard[$toolName] = @{
        total_calls                 = $toolEps.Count
        result_used_count           = $usedCount
        utilization_rate            = if ($toolEps.Count -gt 0) { [math]::Round($usedCount / $toolEps.Count, 2) } else { 0 }
        truncated_count             = $truncCount
        empty_count                 = $emptyCount
        error_count                 = $errorCount
        refinement_chains           = $refinementCount
        sequential_exploration_count = $seqExplCount
        avg_response_size_bytes     = $avgSize
        hints_followed              = $hintsFollowed
        hints_ignored               = $hintsIgnored
        first_useful_call           = $firstUseful
        strategy_changes            = $strategyCount
        auto_corrected_count        = $autoCorrectedCount
        heavy_response_count        = $heavyCount
        total_estimated_tokens      = $totalTokens
        total_body_omitted          = [int]$totalBodyOmitted
    }
}

# ============================================================
# STEP 7: Session summary
# ============================================================

$statusCounts = @{ success = 0; partial = 0; empty = 0; error = 0; noisy = 0 }
foreach ($ep in $episodes) {
    if ($statusCounts.ContainsKey($ep.response_status)) {
        $statusCounts[$ep.response_status]++
    }
}

$redundantCount = @($episodes | Where-Object { $_.tags -contains 'redundant' }).Count
$refinementChainCount = @($episodes | Where-Object { $_.tags -contains 'progressive_refinement' }).Count
$seqExplTotalCount = @($episodes | Where-Object { $_.tags -contains 'sequential_exploration' }).Count

# Top useful tools (by utilization_rate)
$topUseful = @($toolScorecard.GetEnumerator() | Sort-Object { $_.Value.utilization_rate } -Descending | Select-Object -First 3 | ForEach-Object { $_.Key })
$topNoisy = @($toolScorecard.GetEnumerator() | Where-Object { $_.Value.utilization_rate -lt 0.5 } | ForEach-Object { $_.Key })

$summary = @{
    statuses          = $statusCounts
    redundant_calls   = $redundantCount
    refinement_chains = $refinementChainCount
    top_useful_tools  = $topUseful
    top_noisy_tools   = $topNoisy
}

if ($hasSelfAnalysis -and $selfAnalysis.Count -gt 0) {
    $summary['self_analysis'] = $selfAnalysis
}
# Automated recommendations
$recommendations = @()
foreach ($entry in $toolScorecard.GetEnumerator()) {
    $t = $entry.Value
    $toolName = $entry.Key
    if ($t.truncated_count -gt 0) {
        $truncPct = [math]::Round($t.truncated_count / $t.total_calls * 100, 0)
        $recommendations += "[$toolName] $($t.truncated_count)/$($t.total_calls) responses truncated (${truncPct}%). Consider reducing default response size or adding auto-pagination."
    }
    if ($t.refinement_chains -gt 2) {
        $seqExp = $t.sequential_exploration_count
        $recommendations += "[$toolName] $($t.refinement_chains) true refinement chains (+ $seqExp sequential explorations). Model often narrows scope after first call. Consider better default scope or interactive scope discovery."
    }
    if ($t.first_useful_call -gt 1) {
        $recommendations += "[$toolName] First useful call at position $($t.first_useful_call). Consider improving parameter defaults or adding usage hints."
    }
    if ($t.hints_ignored -gt $t.hints_followed -and $t.hints_ignored -gt 0) {
        $recommendations += "[$toolName] Hints ignored $($t.hints_ignored) times vs followed $($t.hints_followed) times. Review hint relevance."
    }
}

# Detect forced enumeration chains: N consecutive calls to same tool where only 'dir' changes
# (model iterating directory-by-directory because tool lacks aggregation)
$forcedEnumerationChains = @()
$enumStart = -1
for ($e = 1; $e -lt $episodes.Count; $e++) {
    $ep = $episodes[$e]
    $prev = $episodes[$e - 1]
    if ($ep.tool -eq $prev.tool -and $ep.server -eq $prev.server -and
        $ep.tags -contains 'progressive_refinement') {
        try {
            $prevP = $prev._call_args | ConvertFrom-Json -ErrorAction Stop
            $currP = $ep._call_args | ConvertFrom-Json -ErrorAction Stop
            # Check: same params except 'dir' changed
            $prevDir = if ($prevP.PSObject.Properties['dir']) { "$($prevP.dir)" } else { '' }
            $currDir = if ($currP.PSObject.Properties['dir']) { "$($currP.dir)" } else { '' }
            $sameTool = $true
            # Compare non-dir params
            $allKeys = @($prevP.PSObject.Properties.Name) + @($currP.PSObject.Properties.Name) | Sort-Object -Unique
            foreach ($key in $allKeys) {
                if ($key -eq 'dir') { continue }
                $pVal = if ($prevP.PSObject.Properties[$key]) { "$($prevP.$key)" } else { '<absent>' }
                $cVal = if ($currP.PSObject.Properties[$key]) { "$($currP.$key)" } else { '<absent>' }
                if ($pVal -ne $cVal) { $sameTool = $false; break }
            }
            if ($sameTool -and $prevDir -ne $currDir -and $prevDir -and $currDir) {
                if ($enumStart -eq -1) { $enumStart = $e - 1 }
            } else {
                if ($enumStart -ne -1 -and ($e - 1 - $enumStart) -ge 2) {
                    $forcedEnumerationChains += @{ start = $enumStart + 1; end = $e; length = $e - $enumStart; tool = $prev.tool }
                }
                $enumStart = -1
            }
        } catch { $enumStart = -1 }
    } else {
        if ($enumStart -ne -1 -and ($e - 1 - $enumStart) -ge 2) {
            $forcedEnumerationChains += @{ start = $enumStart + 1; end = $e; length = $e - $enumStart; tool = $episodes[$e-1].tool }
        }
        $enumStart = -1
    }
}
if ($enumStart -ne -1 -and ($episodes.Count - 1 - $enumStart) -ge 2) {
    $forcedEnumerationChains += @{ start = $enumStart + 1; end = $episodes.Count; length = $episodes.Count - $enumStart; tool = $episodes[-1].tool }
}

# Wasted bytes on truncated-then-refined responses
# Only count as waste if the next call is a RETRY or SCOPE NARROWING of the same query,
# not a completely different query (which is normal exploration workflow)
$wastedBytes = 0
for ($e = 0; $e -lt $episodes.Count; $e++) {
    $ep = $episodes[$e]
    if ($ep.response_status -eq 'partial' -and $e + 1 -lt $episodes.Count) {
        $nextEp = $episodes[$e + 1]
        if ($nextEp.tags -contains 'progressive_refinement' -or $nextEp.tags -contains 'retry') {
            # Check if next call is actually narrowing the SAME query (not a completely different one)
            $isRealWaste = $false
            if ($nextEp.tags -contains 'retry') {
                $isRealWaste = $true  # Exact retry is always waste
            }
            elseif ($nextEp.tool -eq $ep.tool -and $nextEp.server -eq $ep.server) {
                # Same tool progressive refinement: check if params overlap significantly
                try {
                    $prevParams = $ep._call_args | ConvertFrom-Json -ErrorAction Stop
                    $nextParams = $nextEp._call_args | ConvertFrom-Json -ErrorAction Stop
                    # If the same 'name' or 'method' or 'file' param exists in both, it's scope narrowing = waste
                    $sharedKeys = @('name', 'method', 'file', 'terms')
                    foreach ($key in $sharedKeys) {
                        $pVal = if ($prevParams.PSObject.Properties[$key]) { "$($prevParams.$key)" } else { '' }
                        $nVal = if ($nextParams.PSObject.Properties[$key]) { "$($nextParams.$key)" } else { '' }
                        if ($pVal -and $nVal -and $pVal -eq $nVal) {
                            $isRealWaste = $true
                            break
                        }
                    }
                } catch { $isRealWaste = $true }  # If can't parse, assume waste (conservative)
            }
            if ($isRealWaste) {
                $wastedBytes += $ep.response_size_bytes
            }
        }
    }
}
if ($wastedBytes -gt 0) {
    $wastedKB = [math]::Round($wastedBytes / 1024, 1)
    $recommendations += "[efficiency] ${wastedKB}KB wasted on truncated responses that were immediately refined. Server-side auto-narrowing could eliminate this."
}

# Policy violation recommendations
if ($policyViolations.Count -gt 0) {
    $readFileViolations = @($policyViolations | Where-Object { $_.tool -eq 'read_file' })
    $searchFilesViolations = @($policyViolations | Where-Object { $_.tool -eq 'search_files' })
    if ($readFileViolations.Count -gt 0) {
        $violatedExts = @($readFileViolations | ForEach-Object { $_.extension } | Sort-Object -Unique) -join ', .'
        $recommendations += "[policy] $($readFileViolations.Count) read_file call(s) for indexed file types (.$violatedExts) should have used search_definitions includeBody=true."
    }
    if ($searchFilesViolations.Count -gt 0) {
        $recommendations += "[policy] $($searchFilesViolations.Count) search_files call(s) should have used search_grep (search-index MCP was available)."
    }
    $mcpUnavailableViolations = @($policyViolations | Where-Object { -not $_.mcp_available })
    if ($mcpUnavailableViolations.Count -gt 0) {
        $recommendations += "[policy] $($mcpUnavailableViolations.Count) violation(s) occurred while MCP was unavailable (justified fallback)."
    }
}

# Incomplete session warning
if (-not $hasCompletion) {
    $recommendations += '[session] Session ended without attempt_completion — result may not have been delivered to the user.'
}

# Kind mismatch detection
$kindMismatches = @($episodes | Where-Object { $_.tags -contains 'kind_mismatch' })
if ($kindMismatches.Count -gt 0) {
    $kmDetails = ($kindMismatches | ForEach-Object {
        $km = if ($_.ContainsKey('kind_mismatch_details')) { $_.kind_mismatch_details } else { @{ requested_kind = '?'; zero_names = @() } }
        "episode $($_.index): kind=$($km.requested_kind) missed $($km.zero_names -join ',')"
    }) -join '; '
    $recommendations += "[kind_mismatch] $($kindMismatches.Count) kind mismatch(es) caused extra round-trips: $kmDetails. Consider multi-kind filter support (kind='class,interface') or kind mismatch hints."
}

# Data quality complaints
$dqComplaints = @($episodes | Where-Object { $_.tags -contains 'data_quality_complaint' })
if ($dqComplaints.Count -gt 0) {
    $dqTools = @($dqComplaints | ForEach-Object { $_.tool } | Sort-Object -Unique) -join ', '
    $recommendations += "[data_quality] Model flagged data quality issues $($dqComplaints.Count) time(s) in thinking (tools: $dqTools). Review thinking_before content for specific complaints."
}

# Forced enumeration chains
if ($forcedEnumerationChains.Count -gt 0) {
    $totalForcedCalls = ($forcedEnumerationChains | Measure-Object -Property length -Sum).Sum
    $chainDetails = ($forcedEnumerationChains | ForEach-Object { "$($_.tool) episodes $($_.start)-$($_.end) ($($_.length) calls)" }) -join '; '
    $recommendations += "[forced_enumeration] $totalForcedCalls calls in $($forcedEnumerationChains.Count) forced enumeration chain(s): $chainDetails. Model iterated directory-by-directory because tool lacks aggregation. Consider batch dir counting or per-directory file counts in dirsOnly response."
}

# Truncation root causes
$truncationCauses = @{}
foreach ($ep in $episodes) {
    if ($ep.truncation_cause -and $ep.truncation_cause -ne '') {
        foreach ($cause in ($ep.truncation_cause -split ', ')) {
            if (-not $truncationCauses.ContainsKey($cause)) { $truncationCauses[$cause] = 0 }
            $truncationCauses[$cause]++
        }
    }
}

$summary['recommendations'] = $recommendations
$summary['wasted_bytes'] = $wastedBytes
if ($truncationCauses.Count -gt 0) {
    $summary['truncation_root_causes'] = $truncationCauses
}

# ============================================================
# STEP 8: Build output objects (strip internal fields)
# ============================================================

$cleanEpisodes = @()
foreach ($ep in $episodes) {
    $clean = @{
        index               = $ep.index
        server              = $ep.server
        tool                = $ep.tool
        params_summary      = $ep.params_summary
        response_status     = $ep.response_status
        response_size_bytes = $ep.response_size_bytes
        response_summary    = $ep.response_summary
        partial_reason      = $ep.partial_reason
        thinking_before     = $ep.thinking_before
        reaction_after      = $ep.reaction_after
        result_used         = $ep.result_used
        tags                = @($ep.tags)
    }
    if ($ep.param_diff) { $clean['param_diff'] = $ep.param_diff }
    if ($ep.truncation_cause) { $clean['truncation_cause'] = $ep.truncation_cause }
    if ($ep.ContainsKey('kind_mismatch_details') -and $ep.kind_mismatch_details) { $clean['kind_mismatch_details'] = $ep.kind_mismatch_details }
    if ($ep.estimated_tokens -gt 0) { $clean['estimated_tokens'] = $ep.estimated_tokens }
    if ($ep.auto_correction) { $clean['auto_correction'] = $ep.auto_correction }
    if ($ep.body_omitted_count -gt 0) { $clean['body_omitted_count'] = $ep.body_omitted_count }
    if ($ep.term_breakdown) { $clean['term_breakdown'] = $ep.term_breakdown }
    $cleanEpisodes += $clean
}

# Build clean policy violations for output (strip internal fields)
$cleanViolations = @()
foreach ($pv in $policyViolations) {
    $cleanViolations += @{
        turn                  = $pv.turn
        tool                  = $pv.tool
        paths                 = $pv.paths
        extension             = $pv.extension
        mcp_available         = $pv.mcp_available
        suggested_alternative = $pv.suggested_alternative
        reason                = $pv.reason
    }
}

$report = @{
    session        = @{
        source_file            = $fileName
        total_turns            = $totalTurns
        total_mcp_calls        = $episodes.Count
        total_builtin_calls    = $builtinCallCount
        builtin_tools          = $builtinCallDetails
        indexed_extensions     = @($indexedExtensions)
        session_mode           = $sessionMode
        total_estimated_tokens = $totalEstimatedTokens
        auto_corrections_count = $autoCorrections.Count
        phases                 = $phases
    }
    tool_scorecard     = $toolScorecard
    episodes           = $cleanEpisodes
    auto_corrections   = @($autoCorrections)
    policy_violations  = $cleanViolations
    summary            = $summary
}

# ============================================================
# STEP 9: Write JSON
# ============================================================

$jsonOutput = $report | ConvertTo-Json -Depth 10
$jsonOutput | Out-File -FilePath $OutputJson -Encoding UTF8
Write-Host "JSON report written to: $OutputJson"

# ============================================================
# STEP 10: Write Markdown
# ============================================================

$md = [System.Text.StringBuilder]::new()
[void]$md.AppendLine("# MCP Transcript Analysis Report")
[void]$md.AppendLine("")
[void]$md.AppendLine("**Source**: ``$fileName``")
[void]$md.AppendLine("**Total turns**: $totalTurns")
[void]$md.AppendLine("**MCP tool calls**: $($episodes.Count)")
[void]$md.AppendLine("**Built-in tool calls**: $builtinCallCount")
if ($sessionMode) {
    [void]$md.AppendLine("**Session mode**: $sessionMode")
}
if ($totalEstimatedTokens -gt 0) {
    [void]$md.AppendLine("**Total MCP response tokens**: ~$totalEstimatedTokens (~$([math]::Round($totalEstimatedTokens / 1000, 1))K)")
}
if ($autoCorrections.Count -gt 0) {
    [void]$md.AppendLine("**Auto-corrections**: $($autoCorrections.Count)")
}
[void]$md.AppendLine("")

# Phases
[void]$md.AppendLine("## Session Phases")
[void]$md.AppendLine("")
[void]$md.AppendLine("| Phase | Turns | Details |")
[void]$md.AppendLine("|-------|-------|---------|")
[void]$md.AppendLine("| Setup | $setupTurns | $($phases.setup.description) |")
[void]$md.AppendLine("| Exploration | $explorationTurns | $($episodes.Count) MCP calls |")
$synthDetails = if ($hasCompletion) { "completion: yes" } else { "completion: no" }
if ($hasSelfAnalysis) { $synthDetails += ", self-analysis: yes" }
[void]$md.AppendLine("| Synthesis | $synthesisTurns | $synthDetails |")
[void]$md.AppendLine("")

# Tool Quality Scorecard
[void]$md.AppendLine("## Tool Quality Scorecard")
[void]$md.AppendLine("")
[void]$md.AppendLine("| Tool | Calls | Used | Util% | Truncated | Empty | Errors | Refine | SeqExpl | Heavy | AutoCorr | Tokens |")
[void]$md.AppendLine("|------|-------|------|-------|-----------|-------|--------|--------|---------|-------|----------|--------|")
foreach ($entry in ($toolScorecard.GetEnumerator() | Sort-Object { $_.Value.total_calls } -Descending)) {
    $t = $entry.Value
    $utilPct = [math]::Round($t.utilization_rate * 100, 0)
    $tokenK = if ($t.total_estimated_tokens -gt 0) { "~$([math]::Round($t.total_estimated_tokens / 1000, 1))K" } else { '-' }
    [void]$md.AppendLine("| $($entry.Key) | $($t.total_calls) | $($t.result_used_count) | ${utilPct}% | $($t.truncated_count) | $($t.empty_count) | $($t.error_count) | $($t.refinement_chains) | $($t.sequential_exploration_count) | $($t.heavy_response_count) | $($t.auto_corrected_count) | $tokenK |")
}
[void]$md.AppendLine("")

# Status Summary
[void]$md.AppendLine("## Response Status Summary")
[void]$md.AppendLine("")
[void]$md.AppendLine("| Status | Count |")
[void]$md.AppendLine("|--------|-------|")
foreach ($s in $statusCounts.GetEnumerator()) {
    [void]$md.AppendLine("| $($s.Key) | $($s.Value) |")
}
[void]$md.AppendLine("")
if ($redundantCount -gt 0) {
    [void]$md.AppendLine("**Redundant calls**: $redundantCount")
}
if ($refinementChainCount -gt 0) {
    [void]$md.AppendLine("**Progressive refinement chains**: $refinementChainCount")
}
if ($seqExplTotalCount -gt 0) {
    [void]$md.AppendLine("**Sequential explorations**: $seqExplTotalCount")
}
[void]$md.AppendLine("")

# Episodes
[void]$md.AppendLine("## Episodes")
[void]$md.AppendLine("")
foreach ($ep in $cleanEpisodes) {
    $statusEmoji = switch ($ep.response_status) {
        'success' { '✅' }
        'partial' { '⚠️' }
        'empty'   { '🔲' }
        'error'   { '❌' }
        'noisy'   { '📢' }
        default   { '❓' }
    }
    $usedMark = if ($ep.result_used) { '✅' } else { '❌' }
    $tagStr = if ($ep.tags.Count -gt 0) { $ep.tags -join ', ' } else { '-' }

    [void]$md.AppendLine("### Episode $($ep.index): ``$($ep.tool)`` $statusEmoji")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("- **Server**: $($ep.server)")
    [void]$md.AppendLine("- **Params**: $($ep.params_summary)")
    [void]$md.AppendLine("- **Status**: $($ep.response_status) $(if ($ep.partial_reason) { "— $($ep.partial_reason)" })")
    if ($ep.ContainsKey('truncation_cause') -and $ep.truncation_cause) {
        [void]$md.AppendLine("- **Truncation cause**: $($ep.truncation_cause)")
    }
    $sizeStr = "$($ep.response_size_bytes) bytes"
    if ($ep.ContainsKey('estimated_tokens') -and $ep.estimated_tokens -gt 0) {
        $sizeStr += " (~$($ep.estimated_tokens) tokens)"
    }
    [void]$md.AppendLine("- **Response size**: $sizeStr")
    [void]$md.AppendLine("- **Summary**: $($ep.response_summary)")
    if ($ep.ContainsKey('auto_correction') -and $ep.auto_correction) {
        [void]$md.AppendLine("- **Auto-correction**: $($ep.auto_correction.type) — $($ep.auto_correction.reason)")
    }
    if ($ep.ContainsKey('body_omitted_count') -and $ep.body_omitted_count -gt 0) {
        [void]$md.AppendLine("- **Body omitted**: $($ep.body_omitted_count) definitions had body omitted (budget exceeded)")
    }
    if ($ep.ContainsKey('term_breakdown') -and $ep.term_breakdown) {
        $tbParts = @()
        foreach ($prop in $ep.term_breakdown.PSObject.Properties) {
            $tbParts += "$($prop.Name):$($prop.Value)"
        }
        [void]$md.AppendLine("- **Term breakdown**: $($tbParts -join ', ')")
    }
    if ($ep.ContainsKey('kind_mismatch_details') -and $ep.kind_mismatch_details) {
        $km = $ep.kind_mismatch_details
        [void]$md.AppendLine("- **Kind mismatch**: searched as ``$($km.requested_kind)`` but names [$($km.zero_names -join ', ')] returned 0 results. Next call used ``$($km.next_kind)``$(if ($km.recovered_names.Count -gt 0) { " and recovered: $($km.recovered_names -join ', ')" })")
    }
    if ($ep.ContainsKey('param_diff') -and $ep.param_diff) {
        [void]$md.AppendLine("- **Param diff from prev**: $($ep.param_diff)")
    }
    [void]$md.AppendLine("- **Result used**: $usedMark")
    [void]$md.AppendLine("- **Tags**: $tagStr")
    if ($ep.thinking_before) {
        [void]$md.AppendLine("- **Thinking before**: $($ep.thinking_before)")
    }
    if ($ep.reaction_after.type -ne 'none') {
        [void]$md.AppendLine("- **Reaction**: [$($ep.reaction_after.type)] $($ep.reaction_after.content)")
    }
    [void]$md.AppendLine("")
}

# Auto-corrections section
if ($autoCorrections.Count -gt 0) {
    [void]$md.AppendLine("## Auto-corrections")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("Server auto-corrected $($autoCorrections.Count) queries:")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("| Episode | Tool | Type | Reason |")
    [void]$md.AppendLine("|---------|------|------|--------|")
    foreach ($ac in $autoCorrections) {
        [void]$md.AppendLine("| $($ac.episode) | $($ac.tool) | $($ac.type) | $($ac.reason) |")
    }
    [void]$md.AppendLine("")
}

# Token budget section
if ($totalEstimatedTokens -gt 0) {
    [void]$md.AppendLine("## Token Budget")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("**Total tokens consumed by MCP responses**: ~$totalEstimatedTokens (~$([math]::Round($totalEstimatedTokens / 1000, 1))K)")
    $heavyEps = @($cleanEpisodes | Where-Object { $_.ContainsKey('estimated_tokens') -and $_.estimated_tokens -gt 0 } | Sort-Object { $_.estimated_tokens } -Descending | Select-Object -First 5)
    if ($heavyEps.Count -gt 0) {
        [void]$md.AppendLine("")
        [void]$md.AppendLine("**Top 5 heaviest responses:**")
        [void]$md.AppendLine("")
        [void]$md.AppendLine("| Episode | Tool | Tokens | Size (KB) | Status |")
        [void]$md.AppendLine("|---------|------|--------|-----------|--------|")
        foreach ($hep in $heavyEps) {
            $sizeKB = [math]::Round($hep.response_size_bytes / 1024, 1)
            [void]$md.AppendLine("| $($hep.index) | $($hep.tool) | ~$($hep.estimated_tokens) | ${sizeKB}KB | $($hep.response_status) |")
        }
    }
    [void]$md.AppendLine("")
}

# Self-analysis
if ($hasSelfAnalysis) {
    [void]$md.AppendLine("## Model Self-Analysis")
    [void]$md.AppendLine("")
    if ($selfAnalysis.ContainsKey('suboptimal_queries')) {
        [void]$md.AppendLine("- **Suboptimal queries**: $($selfAnalysis.suboptimal_queries)")
    }
    if ($selfAnalysis.ContainsKey('suboptimal_pct')) {
        [void]$md.AppendLine("- **Suboptimal %**: $($selfAnalysis.suboptimal_pct)%")
    }
    if ($selfAnalysis.ContainsKey('improvement_ideas')) {
        [void]$md.AppendLine("- **Improvement ideas**:")
        foreach ($idea in $selfAnalysis.improvement_ideas) {
            [void]$md.AppendLine("  - $idea")
        }
    }
    if ($selfAnalysis.ContainsKey('raw_text')) {
        [void]$md.AppendLine("- **Raw self-analysis**: $($selfAnalysis.raw_text)")
    }
    [void]$md.AppendLine("")
}

# Automated Recommendations
if ($recommendations.Count -gt 0) {
    [void]$md.AppendLine("## Automated Recommendations for search-index improvement")
    [void]$md.AppendLine("")
    foreach ($rec in $recommendations) {
        [void]$md.AppendLine("- $rec")
    }
    [void]$md.AppendLine("")
}

# Truncation Root Causes
if ($truncationCauses.Count -gt 0) {
    [void]$md.AppendLine("## Truncation Root Causes")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("| Cause | Occurrences |")
    [void]$md.AppendLine("|-------|-------------|")
    foreach ($c in ($truncationCauses.GetEnumerator() | Sort-Object Value -Descending)) {
        [void]$md.AppendLine("| $($c.Key) | $($c.Value) |")
    }
    [void]$md.AppendLine("")
}

# Policy Violations
if ($policyViolations.Count -gt 0) {
    [void]$md.AppendLine("## Policy Violations")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("**Indexed extensions detected**: $($indexedExtensions -join ', ')")
    [void]$md.AppendLine("**Total violations**: $($policyViolations.Count)")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("| # | Tool | Extension | MCP Available | Suggested Alternative | File Path |")
    [void]$md.AppendLine("|---|------|-----------|---------------|----------------------|-----------|")
    $pvIdx = 0
    foreach ($pv in $policyViolations) {
        $pvIdx++
        $mcpStatus = if ($pv.mcp_available) { '✅ yes' } else { '❌ no' }
        $filePath = if ($pv.paths.Count -gt 0) { (Summarize-Text ($pv.paths -join ', ') 80) } else { '-' }
        $extDisplay = if ($pv.extension) { ".$($pv.extension)" } else { '-' }
        [void]$md.AppendLine("| $pvIdx | $($pv.tool) | $extDisplay | $mcpStatus | $($pv.suggested_alternative) | $filePath |")
    }
    [void]$md.AppendLine("")
}
elseif ($indexedExtensions.Count -gt 0) {
    [void]$md.AppendLine("## Policy Violations")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("**Indexed extensions detected**: $($indexedExtensions -join ', ')")
    [void]$md.AppendLine("No policy violations detected. ✅")
    [void]$md.AppendLine("")
}

# Waste summary
if ($wastedBytes -gt 0) {
    $wastedKB = [math]::Round($wastedBytes / 1024, 1)
    [void]$md.AppendLine("## Efficiency")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("**Wasted on truncated-then-refined responses**: ${wastedKB}KB")
    [void]$md.AppendLine("")
}

$md.ToString() | Out-File -FilePath $OutputMd -Encoding UTF8
Write-Host "Markdown report written to: $OutputMd"
Write-Host ""
Write-Host "=== Quick Summary ==="
Write-Host "Episodes: $($episodes.Count) MCP calls, $builtinCallCount built-in calls"
Write-Host "Statuses: success=$($statusCounts.success), partial=$($statusCounts.partial), empty=$($statusCounts.empty), error=$($statusCounts.error)"
Write-Host "Policy violations: $($policyViolations.Count)$(if ($indexedExtensions.Count -gt 0) { " (indexed: $($indexedExtensions -join ','))" } else { ' (no indexed extensions detected)' })"
Write-Host "Self-analysis: $(if ($hasSelfAnalysis) { 'yes' } else { 'no' })"
if ($sessionMode) { Write-Host "Session mode: $sessionMode" }
if ($totalEstimatedTokens -gt 0) { Write-Host "Total MCP response tokens: ~$totalEstimatedTokens (~$([math]::Round($totalEstimatedTokens / 1000, 1))K)" }
if ($autoCorrections.Count -gt 0) { Write-Host "Auto-corrections: $($autoCorrections.Count)" }