param(
    [switch] $SelfTest
)

$ErrorActionPreference = 'Stop'

$FormatterCommand = 'rust' + 'fmt'
$CargoCommand = 'cargo'
$CargoFormatSubcommand = 'f' + 'mt'
$DeniedReason = 'Rust formatter commands are forbidden in this workspace. Preserve existing formatting and line endings.'
$CommandFieldNames = @('command', 'cmd', 'shellCommand', 'commandLine', 'script')

function Get-FieldValue($object, [string[]] $names) {
    if ($null -eq $object) {
        return $null
    }

    foreach ($name in $names) {
        if ($object -is [System.Collections.IDictionary] -and $object.Contains($name)) {
            return $object[$name]
        }

        $property = $object.PSObject.Properties[$name]
        if ($null -ne $property) {
            return $property.Value
        }
    }
    return $null
}

function Convert-CandidateToText($value) {
    if ($null -eq $value) {
        return ''
    }
    if ($value -is [string]) {
        return $value
    }
    return ($value | ConvertTo-Json -Depth 100 -Compress)
}

function Get-CommandTextCandidates($value, [int] $depth = 0) {
    if ($null -eq $value -or $depth -gt 8) {
        return
    }

    if ($value -is [string]) {
        return
    }

    if ($value -is [System.Collections.IDictionary]) {
        foreach ($key in $value.Keys) {
            $keyText = [string] $key
            $entryValue = $value[$key]
            if ($CommandFieldNames -contains $keyText) {
                Convert-CandidateToText $entryValue
            } elseif ($entryValue -isnot [string]) {
                Get-CommandTextCandidates $entryValue ($depth + 1)
            }
        }
        return
    }

    if ($value -is [System.Collections.IEnumerable] -and $value -isnot [string]) {
        foreach ($item in $value) {
            Get-CommandTextCandidates $item ($depth + 1)
        }
        return
    }

    foreach ($property in $value.PSObject.Properties) {
        if ($CommandFieldNames -contains $property.Name) {
            Convert-CandidateToText $property.Value
        } elseif ($property.Value -isnot [string]) {
            Get-CommandTextCandidates $property.Value ($depth + 1)
        }
    }
}

function Remove-QuotedShellContent([string] $text) {
    $withoutHereStrings = [regex]::Replace($text, '(?s)@''.*?''@', ' ')
    $withoutHereStrings = [regex]::Replace($withoutHereStrings, '(?s)@".*?"@', ' ')

    $builder = [System.Text.StringBuilder]::new($withoutHereStrings.Length)
    $quote = [char]0
    foreach ($ch in $withoutHereStrings.ToCharArray()) {
        if ($quote -ne [char]0) {
            if ($ch -eq $quote) {
                $quote = [char]0
            }
            [void] $builder.Append(' ')
            continue
        }

        if ($ch -eq [char]34 -or $ch -eq [char]39) {
            $quote = $ch
            [void] $builder.Append(' ')
        } else {
            [void] $builder.Append($ch)
        }
    }
    $builder.ToString()
}

function Convert-ShellArgumentText([string] $argument) {
    $trimmed = $argument.Trim()
    if ($trimmed.Length -ge 2) {
        $first = $trimmed[0]
        $last = $trimmed[$trimmed.Length - 1]
        if (($first -eq [char]34 -and $last -eq [char]34) -or ($first -eq [char]39 -and $last -eq [char]39)) {
            return $trimmed.Substring(1, $trimmed.Length - 2).Replace("''", "'")
        }
    }
    return $trimmed
}

function Get-ShellWrapperPayloads([string] $commandText) {
    $searchText = [regex]::Replace($commandText, '(?s)@''.*?''@', ' ')
    $searchText = [regex]::Replace($searchText, '(?s)@".*?"@', ' ')
    $argumentPattern = '(?:"(?:`.|[^"])*"|''(?:''''|[^''])*''|[^\r\n;&|]+)'
    $powerShellPattern = '(?im)(^|[\r\n;&|])\s*(?:&\s*)?(?:\S+[\\/])?(?:pwsh|powershell)(?:\.exe)?(?:\s+(?:-\S+|\S+))*?\s+-(?:Command|c)\s+(?<payload>' + $argumentPattern + ')'
    $bashPattern = '(?im)(^|[\r\n;&|])\s*(?:\S+[\\/])?(?:bash|sh)(?:\.exe)?(?:\s+-[^\s]*c[^\s]*)\s+(?<payload>' + $argumentPattern + ')'
    $cmdPattern = '(?im)(^|[\r\n;&|])\s*(?:\S+[\\/])?cmd(?:\.exe)?(?:\s+/d)?\s+/(?:c|k)\s+(?<payload>[^\r\n;&|]+)'

    foreach ($pattern in @($powerShellPattern, $bashPattern, $cmdPattern)) {
        foreach ($match in [regex]::Matches($searchText, $pattern)) {
            Convert-ShellArgumentText $match.Groups['payload'].Value
        }
    }
}

function Test-ForbiddenFormatterInvocation([string] $commandText, [int] $depth = 0) {
    $shellText = Remove-QuotedShellContent $commandText
    $commandStart = '(^|[\r\n;&|])\s*(?:&\s*)?(?:\S+[\\/])?'
    $commandEnd = '(?=$|[\s\r\n;&|])'
    $formatterPattern = '(?im)' + $commandStart + [regex]::Escape($FormatterCommand) + '(\.exe)?' + $commandEnd
    $cargoPattern = '(?im)' + $commandStart + [regex]::Escape($CargoCommand) + '(\.exe)?(?:\s+(?:\+\S+|-\S+))*\s+' + [regex]::Escape($CargoFormatSubcommand) + $commandEnd

    if ($shellText -match $formatterPattern -or $shellText -match $cargoPattern) {
        return $true
    }

    if ($depth -ge 4) {
        return $false
    }

    foreach ($payload in @(Get-ShellWrapperPayloads $commandText)) {
        if (Test-ForbiddenFormatterInvocation $payload ($depth + 1)) {
            return $true
        }
    }

    return $false
}

function Test-EventShouldBeDenied($hookEvent) {
    $toolInput = Get-FieldValue $hookEvent @('tool_input', 'toolInput', 'parameters', 'arguments', 'args', 'input')
    foreach ($commandText in @(Get-CommandTextCandidates $toolInput)) {
        if (Test-ForbiddenFormatterInvocation $commandText) {
            return $true
        }
    }
    return $false
}

function Assert-Decision($name, $hookEvent, [bool] $shouldDeny) {
    $actual = Test-EventShouldBeDenied ([pscustomobject] $hookEvent)
    if ($actual -ne $shouldDeny) {
        throw "SelfTest failed: $name expected deny=$shouldDeny actual=$actual"
    }
}

if ($SelfTest) {
    $formatterExe = $FormatterCommand + '.exe'
    $quotedPrCommand = 'gh pr create --body ' + [char]34 + "mentions $FormatterCommand and $CargoCommand $CargoFormatSubcommand" + [char]34
    $hereStringPrCommand = '$body = @''' + "`n$FormatterCommand`n" + '''@' + "`ngh pr create --body `$body"
    $powerShellWrapperCommand = 'pwsh -NoProfile -Command ' + [char]34 + "$CargoCommand $CargoFormatSubcommand --check" + [char]34
    $bashWrapperCommand = 'bash -lc ' + [char]39 + "$CargoCommand $CargoFormatSubcommand --check" + [char]39
    $cmdWrapperCommand = "cmd /c $formatterExe src/lib.rs"
    $quotedWrapperPrCommand = 'gh pr create --body ' + [char]34 + 'pwsh -Command ' + [char]39 + "$CargoCommand $CargoFormatSubcommand" + [char]39 + [char]34
    $hereStringWrapperPrCommand = '$body = @''' + "`npwsh -Command $CargoCommand $CargoFormatSubcommand`n" + '''@' + "`ngh pr create --body `$body"
    Assert-Decision 'blocks direct formatter command' @{ tool_input = @{ command = $FormatterCommand } } $true
    Assert-Decision 'blocks formatter executable' @{ tool_input = @{ command = $formatterExe } } $true
    Assert-Decision 'blocks cargo format subcommand' @{ tool_input = @{ command = "$CargoCommand $CargoFormatSubcommand" } } $true
    Assert-Decision 'blocks cargo executable format subcommand' @{ tool_input = @{ command = "$CargoCommand.exe $CargoFormatSubcommand --check" } } $true
    Assert-Decision 'blocks powershell wrapper format command' @{ tool_input = @{ command = $powerShellWrapperCommand } } $true
    Assert-Decision 'blocks bash wrapper format command' @{ tool_input = @{ command = $bashWrapperCommand } } $true
    Assert-Decision 'blocks cmd wrapper formatter command' @{ tool_input = @{ command = $cmdWrapperCommand } } $true
    Assert-Decision 'allows search term payloads' @{ tool_input = @{ terms = @($FormatterCommand); file = @('block-' + $FormatterCommand + '.ps1') } } $false
    Assert-Decision 'allows quoted PR text' @{ tool_input = @{ command = $quotedPrCommand } } $false
    Assert-Decision 'allows here-string PR text' @{ tool_input = @{ command = $hereStringPrCommand } } $false
    Assert-Decision 'allows quoted wrapper PR text' @{ tool_input = @{ command = $quotedWrapperPrCommand } } $false
    Assert-Decision 'allows here-string wrapper PR text' @{ tool_input = @{ command = $hereStringWrapperPrCommand } } $false
    Assert-Decision 'allows wrapper search term payloads' @{ tool_input = @{ terms = @($powerShellWrapperCommand) } } $false
    'SelfTest passed'
    exit 0
}

$payload = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($payload)) {
    exit 0
}

try {
    $hookEvent = $payload | ConvertFrom-Json -Depth 100
} catch {
    exit 0
}

if (Test-EventShouldBeDenied $hookEvent) {
    @{
        hookSpecificOutput = @{
            hookEventName = 'PreToolUse'
            permissionDecision = 'deny'
            permissionDecisionReason = $DeniedReason
        }
        systemMessage = 'Blocked forbidden Rust formatter invocation.'
    } | ConvertTo-Json -Depth 10 -Compress
}

