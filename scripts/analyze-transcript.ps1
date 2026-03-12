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
    return @(,$tools)
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
        # Missing kind filter
        if (-not $args_.kind) { $causes += "no_kind_filter" }
        # Missing name filter
        if (-not $args_.name) { $causes += "no_name_filter" }
    }
    catch {}

    # Fallback: infer response_size_limit from response size when no specific markers found
    if ($episode.response_size_bytes -gt 15000 -and $causes -notcontains 'response_size_limit') {
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

    if ($turn.role -ne 'assistant') { continue }

    # Count built-in tools
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

        # Compute param_diff from previous episode (if same tool)
        $paramDiff = ''
        if ($episodes.Count -gt 0) {
            $prevEp = $episodes[$episodes.Count - 1]
            if ($prevEp.tool -eq $call.tool -and $prevEp.server -eq $call.server) {
                $paramDiff = Get-ParamDiff $prevEp._call_args $call.args
            }
        }

        $newEp = @{
            index             = $episodes.Count + 1
            server            = $call.server
            tool              = $call.tool
            params_summary    = Normalize-Params $call.args
            response_status   = $status.status
            response_size_bytes = $responseSize
            response_summary  = $responseSummary
            partial_reason    = $status.reason
            thinking_before   = $thinkingBefore
            reaction_after    = $reactionAfter
            result_used       = $false
            tags              = [System.Collections.Generic.List[string]]::new()
            param_diff        = $paramDiff
            truncation_cause  = ''
            _result_raw       = $resultContent
            _call_args        = $call.args
            _hints            = @(Extract-Hints $resultContent)
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

    # progressive_refinement vs retry
    if ($e -gt 0) {
        $prev = $episodes[$e - 1]
        if ($prev.tool -eq $ep.tool -and $prev.server -eq $ep.server) {
            # Compare args: if different -> refinement, if same -> retry
            if ($prev._call_args -eq $ep._call_args) {
                $ep.tags.Add('retry')
            }
            else {
                $ep.tags.Add('progressive_refinement')
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
    $strategyCount = @($toolEps | Where-Object { $_.tags -contains 'strategy_change' }).Count
    $hintsFollowed = @($toolEps | Where-Object { $_.tags -contains 'hint_followed' }).Count
    $hintsIgnored = @($toolEps | Where-Object { $_.tags -contains 'hint_ignored' }).Count
    $avgSize = [math]::Round(($toolEps | Measure-Object -Property response_size_bytes -Average).Average, 0)

    # first_useful_call: ordinal within this tool's calls
    $firstUseful = 0
    for ($j = 0; $j -lt $toolEps.Count; $j++) {
        if ($toolEps[$j].result_used) { $firstUseful = $j + 1; break }
    }

    $toolScorecard[$toolName] = @{
        total_calls           = $toolEps.Count
        result_used_count     = $usedCount
        utilization_rate      = if ($toolEps.Count -gt 0) { [math]::Round($usedCount / $toolEps.Count, 2) } else { 0 }
        truncated_count       = $truncCount
        empty_count           = $emptyCount
        error_count           = $errorCount
        refinement_chains     = $refinementCount
        avg_response_size_bytes = $avgSize
        hints_followed        = $hintsFollowed
        hints_ignored         = $hintsIgnored
        first_useful_call     = $firstUseful
        strategy_changes      = $strategyCount
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
        $recommendations += "[$toolName] $($t.refinement_chains) refinement chains. Model often narrows scope after first call. Consider better default scope or interactive scope discovery."
    }
    if ($t.first_useful_call -gt 1) {
        $recommendations += "[$toolName] First useful call at position $($t.first_useful_call). Consider improving parameter defaults or adding usage hints."
    }
    if ($t.hints_ignored -gt $t.hints_followed -and $t.hints_ignored -gt 0) {
        $recommendations += "[$toolName] Hints ignored $($t.hints_ignored) times vs followed $($t.hints_followed) times. Review hint relevance."
    }
}

# Wasted bytes on truncated-then-refined responses
$wastedBytes = 0
for ($e = 0; $e -lt $episodes.Count; $e++) {
    $ep = $episodes[$e]
    if ($ep.response_status -eq 'partial' -and $e + 1 -lt $episodes.Count) {
        if ($episodes[$e + 1].tags -contains 'progressive_refinement') {
            $wastedBytes += $ep.response_size_bytes
        }
    }
}
if ($wastedBytes -gt 0) {
    $wastedKB = [math]::Round($wastedBytes / 1024, 1)
    $recommendations += "[efficiency] ${wastedKB}KB wasted on truncated responses that were immediately refined. Server-side auto-narrowing could eliminate this."
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
    $cleanEpisodes += $clean
}

$report = @{
    session        = @{
        source_file        = $fileName
        total_turns        = $totalTurns
        total_mcp_calls    = $episodes.Count
        total_builtin_calls = $builtinCallCount
        builtin_tools      = $builtinCallDetails
        phases             = $phases
    }
    tool_scorecard = $toolScorecard
    episodes       = $cleanEpisodes
    summary        = $summary
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
[void]$md.AppendLine("| Tool | Calls | Used | Util% | Truncated | Empty | Errors | Refinements | 1st Useful |")
[void]$md.AppendLine("|------|-------|------|-------|-----------|-------|--------|-------------|------------|")
foreach ($entry in ($toolScorecard.GetEnumerator() | Sort-Object { $_.Value.total_calls } -Descending)) {
    $t = $entry.Value
    $utilPct = [math]::Round($t.utilization_rate * 100, 0)
    [void]$md.AppendLine("| $($entry.Key) | $($t.total_calls) | $($t.result_used_count) | ${utilPct}% | $($t.truncated_count) | $($t.empty_count) | $($t.error_count) | $($t.refinement_chains) | $($t.first_useful_call) |")
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
    [void]$md.AppendLine("- **Response size**: $($ep.response_size_bytes) bytes")
    [void]$md.AppendLine("- **Summary**: $($ep.response_summary)")
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
Write-Host "Self-analysis: $(if ($hasSelfAnalysis) { 'yes' } else { 'no' })"