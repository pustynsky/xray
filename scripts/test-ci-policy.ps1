$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'ci-policy.ps1')

$root = Join-Path ([IO.Path]::GetTempPath()) ('xray-ci-policy-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$oldGlobal = $env:GIT_CONFIG_GLOBAL
$oldSystem = $env:GIT_CONFIG_NOSYSTEM
$env:GIT_CONFIG_GLOBAL = Join-Path $root 'global.gitconfig'
$env:GIT_CONFIG_NOSYSTEM = '1'
$script:passes = 0
$script:failures = 0

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Assert-Throws {
    param([scriptblock]$Body)
    $threw = $false
    try { & $Body }
    catch { $threw = $true }
    Assert-True $threw 'Expected an explicit error.'
}

function Test-Case {
    param([string]$Name, [scriptblock]$Body)
    try {
        & $Body
        $script:passes++
        Write-Host "PASS $Name"
    }
    catch {
        $script:failures++
        Write-Host "FAIL $Name`: $($_.Exception.Message)" -ForegroundColor Red
    }
}

function Write-FixtureFile {
    param([string]$Repo, [string]$Path, [string]$Text = 'fixture')
    $target = Join-Path $Repo $Path
    New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
    [IO.File]::WriteAllText($target, $Text)
}

function Commit-Fixture {
    param([string]$Repo)
    $null = Invoke-CiGit $Repo @('add', '-A')
    $null = Invoke-CiGit $Repo @('commit', '--quiet', '--allow-empty', '-m', 'fixture')
    return (Invoke-CiGit $Repo @('rev-parse', 'HEAD'))
}

function New-Fixture {
    $repo = Join-Path $root ([Guid]::NewGuid().ToString('N') + ' repo with spaces')
    $null = Invoke-CiGit $root @('init', '--quiet', '--initial-branch=main', $repo)
    foreach ($pair in @(
        @('user.name', 'CI Policy Test'), @('user.email', 'ci-test@example.invalid'),
        @('commit.gpgSign', 'false'), @('core.autocrlf', 'false'),
        @('core.hooksPath', (Join-Path $root 'no-hooks'))
    )) {
        $null = Invoke-CiGit $repo @('config', $pair[0], $pair[1])
    }
    Write-FixtureFile $repo 'README.md' 'baseline'
    Write-FixtureFile $repo 'docs/architecture.md' 'baseline'
    Write-FixtureFile $repo 'src/example.rs' 'baseline'
    return [pscustomobject]@{ Repo = $repo; Base = (Commit-Fixture $repo) }
}

function New-Needs {
    param([string]$Mode = 'full')
    $needs = [ordered]@{
        classify = [pscustomobject]@{
            result = 'success'
            outputs = [pscustomobject]@{ mode = $Mode }
        }
    }
    foreach ($job in @('setup', 'clippy', 'test-windows', 'test-ubuntu',
        'setup-integration', 'feature-matrix', 'audit', 'deny')) {
        $result = if ($Mode -eq 'docs') { 'skipped' } else { 'success' }
        $needs[$job] = [pscustomobject]@{ result = $result }
    }
    return [pscustomobject]$needs
}

try {
    foreach ($path in @('README.md', 'CHANGELOG.md', 'LICENSE-MIT', 'docs/architecture.md',
        'docs/e2e/README.md', '.github/agents/Explore-with-Xray.agent.md',
        '.github/skills/release/SKILL.md', '.github/instructions/review-agent.instructions.md')) {
        Test-Case "allowed document: $path" {
            $f = New-Fixture
            Write-FixtureFile $f.Repo $path 'updated'
            $head = Commit-Fixture $f.Repo
            Assert-True ((Get-CiMode pull_request $f.Base $head $f.Repo) -ceq 'docs') 'Expected docs mode.'
        }
    }
    foreach ($path in @('src/example.rs', 'Cargo.toml', 'Cargo.lock', 'build.rs',
        'tests/fixture.md', 'docs/new-guide.md', 'docs/tool.ps1', '.gitignore',
        '.github/workflows/ci.yml', '.github/hooks/check.ps1',
        '.github/agents/new-agent.agent.md', '.github/skills/release/publish.ps1',
        'scripts/ci-policy.ps1', 'scripts/setup-xray.ps1', 'docs/unusual name.md')) {
        Test-Case "unknown or executable path: $path" {
            $f = New-Fixture
            Write-FixtureFile $f.Repo $path 'updated'
            $head = Commit-Fixture $f.Repo
            Assert-True ((Get-CiMode pull_request $f.Base $head $f.Repo) -ceq 'full') 'Expected full mode.'
        }
    }
    Test-Case 'mixed changes require full CI' {
        $f = New-Fixture
        Write-FixtureFile $f.Repo 'README.md' 'updated'
        Write-FixtureFile $f.Repo 'src/example.rs' 'updated'
        $head = Commit-Fixture $f.Repo
        Assert-True ((Get-CiMode pull_request $f.Base $head $f.Repo) -ceq 'full') 'Code change was skipped.'
    }
    Test-Case 'classification includes earlier PR commits' {
        $f = New-Fixture
        Write-FixtureFile $f.Repo 'src/example.rs' 'updated'
        $null = Commit-Fixture $f.Repo
        Write-FixtureFile $f.Repo 'README.md' 'updated'
        $head = Commit-Fixture $f.Repo
        Assert-True ((Get-CiMode pull_request $f.Base $head $f.Repo) -ceq 'full') 'Earlier code change was skipped.'
    }
    foreach ($source in @('src/example.rs', 'docs/architecture.md')) {
        Test-Case "deletion: $source" {
            $f = New-Fixture
            Remove-Item -LiteralPath (Join-Path $f.Repo $source)
            $head = Commit-Fixture $f.Repo
            $expected = if ($source -eq 'src/example.rs') { 'full' } else { 'docs' }
            Assert-True ((Get-CiMode pull_request $f.Base $head $f.Repo) -ceq $expected) 'Deletion misclassified.'
        }
    }
    foreach ($rename in @(
        @('src/example.rs', 'docs/mcp-guide.md', 'full'),
        @('docs/architecture.md', 'docs/new-guide.md', 'full'),
        @('docs/architecture.md', 'docs/mcp-guide.md', 'docs')
    )) {
        Test-Case "rename: $($rename[0]) to $($rename[1])" {
            $f = New-Fixture
            Move-Item -LiteralPath (Join-Path $f.Repo $rename[0]) -Destination (Join-Path $f.Repo $rename[1])
            $head = Commit-Fixture $f.Repo
            Assert-True ((Get-CiMode pull_request $f.Base $head $f.Repo) -ceq $rename[2]) 'Rename misclassified.'
        }
    }
    Test-Case 'base-only changes are excluded through merge-base' {
        $f = New-Fixture
        Write-FixtureFile $f.Repo 'README.md' 'updated'
        $head = Commit-Fixture $f.Repo
        $null = Invoke-CiGit $f.Repo @('checkout', '--quiet', '-b', 'base-side', $f.Base)
        Write-FixtureFile $f.Repo 'src/example.rs' 'base-only'
        $base = Commit-Fixture $f.Repo
        Assert-True ((Get-CiMode pull_request $base $head $f.Repo) -ceq 'docs') 'Base-only code change contaminated PR diff.'
    }
    Test-Case 'empty diff conservatively requires full CI' {
        $f = New-Fixture
        Assert-True ((Get-CiMode pull_request $f.Base $f.Base $f.Repo) -ceq 'full') 'Empty diff became docs-only.'
    }
    Test-Case 'push and manual events always select full CI' {
        Assert-True ((Get-CiMode push) -ceq 'full') 'Push must be full.'
        Assert-True ((Get-CiMode workflow_dispatch) -ceq 'full') 'Manual run must be full.'
        Assert-Throws { Get-CiMode unknown }
    }
    Test-Case 'invalid and missing SHAs fail explicitly' {
        $f = New-Fixture
        Assert-Throws { Get-CiMode pull_request '' $f.Base $f.Repo }
        Assert-Throws { Get-CiMode pull_request '--all' $f.Base $f.Repo }
        Assert-Throws { Get-CiMode pull_request ('0' * 40) $f.Base $f.Repo }
        $blob = Invoke-CiGit $f.Repo @('rev-parse', 'HEAD:README.md')
        Assert-Throws { Get-CiMode pull_request $blob $f.Base $f.Repo }
        Assert-Throws { Get-CiMode pull_request $f.Base $f.Base (Join-Path $root 'missing') }
    }
    Test-Case 'unrelated histories fail instead of skipping CI' {
        $f = New-Fixture
        $null = Invoke-CiGit $f.Repo @('checkout', '--quiet', '--orphan', 'unrelated')
        Write-FixtureFile $f.Repo 'README.md' 'unrelated root'
        $head = Commit-Fixture $f.Repo
        Assert-Throws { Get-CiMode pull_request $f.Base $head $f.Repo }
    }
    Test-Case 'ambiguous merge bases fail instead of selecting an arbitrary diff' {
        $f = New-Fixture
        Write-FixtureFile $f.Repo 'README.md' 'first branch'
        $first = Commit-Fixture $f.Repo
        $null = Invoke-CiGit $f.Repo @('checkout', '--quiet', '-b', 'second', $f.Base)
        Write-FixtureFile $f.Repo 'src/example.rs' 'second branch'
        $second = Commit-Fixture $f.Repo
        $tree = Invoke-CiGit $f.Repo @('rev-parse', "$first^{tree}")
        $base = Invoke-CiGit $f.Repo @('commit-tree', $tree, '-p', $first, '-p', $second, '-m', 'first merge')
        $head = Invoke-CiGit $f.Repo @('commit-tree', $tree, '-p', $second, '-p', $first, '-m', 'second merge')
        Assert-Throws { Get-CiMode pull_request $base $head $f.Repo }
    }

    foreach ($mode in @('full', 'docs')) {
        Test-Case "gate accepts the exact $mode result set" {
            Assert-CiResults (New-Needs $mode)
        }
        foreach ($job in @('setup', 'clippy', 'test-windows', 'test-ubuntu',
            'setup-integration', 'feature-matrix', 'audit', 'deny')) {
            foreach ($result in @('failure', 'cancelled', '')) {
                Test-Case "gate rejects $mode / $job / '$result'" {
                    $needs = New-Needs $mode
                    $needs.$job.result = $result
                    Assert-Throws { Assert-CiResults $needs }
                }
            }
            Test-Case "gate rejects unexpected execution or skip: $mode / $job" {
                $needs = New-Needs $mode
                $needs.$job.result = if ($mode -eq 'docs') { 'success' } else { 'skipped' }
                Assert-Throws { Assert-CiResults $needs }
            }
        }
    }
    foreach ($result in @('failure', 'cancelled', 'skipped', '')) {
        Test-Case "gate rejects unsuccessful classifier: '$result'" {
            $needs = New-Needs
            $needs.classify.result = $result
            Assert-Throws { Assert-CiResults $needs }
        }
    }
    Test-Case 'gate rejects missing, extra or malformed dependencies and mode' {
        $needs = New-Needs
        $needs.PSObject.Properties.Remove('setup')
        Assert-Throws { Assert-CiResults $needs }
        $needs = New-Needs
        $needs.PSObject.Properties.Remove('classify')
        Assert-Throws { Assert-CiResults $needs }
        $needs = New-Needs
        $needs | Add-Member -NotePropertyName extra -NotePropertyValue ([pscustomobject]@{ result = 'success' })
        Assert-Throws { Assert-CiResults $needs }
        $needs = New-Needs
        $needs.classify.outputs.mode = 'unknown'
        Assert-Throws { Assert-CiResults $needs }
        $needs.classify.outputs = $null
        Assert-Throws { Assert-CiResults $needs }
        Assert-Throws { Assert-CiResults ([pscustomobject]@{}) }
    }
}
finally {
    $env:GIT_CONFIG_GLOBAL = $oldGlobal
    $env:GIT_CONFIG_NOSYSTEM = $oldSystem
    Remove-Item -LiteralPath $root -Recurse -Force
}

Write-Host "CI policy cases passed: $script:passes; failed: $script:failures"
if ($script:failures -gt 0 -or $script:passes -eq 0) { exit 1 }
exit 0
