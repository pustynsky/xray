function Invoke-CiGit {
    param([string]$RepoRoot, [string[]]$Arguments)

    $output = @(& git -C $RepoRoot @Arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "CI policy Git command failed (exit $LASTEXITCODE): git $($Arguments -join ' ')"
    }
    return $output
}

function Get-CiMode {
    param(
        [Parameter(Mandatory)][ValidateSet('pull_request', 'push', 'workflow_dispatch')]
        [string]$EventName,
        [string]$BaseSha,
        [string]$HeadSha,
        [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot)
    )

    if ($EventName -cne 'pull_request') { return 'full' }
    foreach ($sha in @($BaseSha, $HeadSha)) {
        if ($sha -cnotmatch '\A(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})\z') {
            throw 'PR classification requires full base and head commit SHAs.'
        }
        $null = Invoke-CiGit $RepoRoot @('rev-parse', '--verify', "$sha^{commit}")
    }

    $mergeBase = @(Invoke-CiGit $RepoRoot @('merge-base', '--all', $BaseSha, $HeadSha))
    if ($mergeBase.Count -ne 1 -or $mergeBase[0] -cnotmatch '\A(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})\z') {
        throw 'PR classification requires one unambiguous merge base.'
    }
    # Disable rename detection so both the removed and added paths are classified.
    $paths = @(Invoke-CiGit $RepoRoot @(
        'diff', '--no-ext-diff', '--no-textconv', '--name-only', '--no-renames',
        $mergeBase[0], $HeadSha, '--'
    ))
    if ($paths.Count -eq 0) { return 'full' }

    $allowedPaths = @(
        'README.md'
        'CHANGELOG.md'
        'LICENSE-APACHE'
        'LICENSE-MIT'
        'docs/architecture.md'
        'docs/benchmarks.md'
        'docs/cli-reference.md'
        'docs/concurrency.md'
        'docs/development.md'
        'docs/di-support.md'
        'docs/e2e-test-plan.md'
        'docs/e2e/README.md'
        'docs/e2e/cli-tests.md'
        'docs/e2e/git-tests.md'
        'docs/e2e/infrastructure-tests.md'
        'docs/e2e/language-tests.md'
        'docs/e2e/mcp-callers-tests.md'
        'docs/e2e/mcp-definitions-tests.md'
        'docs/e2e/mcp-fast-edit-tests.md'
        'docs/e2e/mcp-grep-tests.md'
        'docs/installation.md'
        'docs/mcp-filter-design.md'
        'docs/mcp-guide.md'
        'docs/measurements/ac4-literal-extraction-bench.md'
        'docs/storage.md'
        'docs/test-coverage-audit.md'
        'docs/tradeoffs.md'
        'docs/use-cases.md'
        '.github/agents/Explore-with-Xray.agent.md'
        '.github/agents/xray-code-reviewer.agent.md'
        '.github/agents/xray-script-reviewer.agent.md'
        '.github/instructions/review-agent.instructions.md'
        '.github/skills/release/SKILL.md'
        '.github/skills/review/SKILL.md'
        '.github/skills/xray-review-playbook/SKILL.md'
        '.github/skills/xray-review-playbook/output-format.md'
        '.github/skills/xray-review-playbook/rust-hazards.md'
        '.github/skills/xray-review-playbook/script-hazards.md'
        '.github/skills/xray-review-playbook/validation-matrix.md'
    )
    foreach ($path in $paths) {
        if ($allowedPaths -cnotcontains $path) { return 'full' }
    }
    return 'docs'
}

function Assert-CiResults {
    param([Parameter(Mandatory)][psobject]$Needs)

    $classification = $Needs.PSObject.Properties['classify']
    if (-not $classification -or $classification.Value.result -cne 'success') {
        throw 'CI classification did not succeed.'
    }
    $mode = $classification.Value.outputs.mode
    if ($mode -cnotin @('full', 'docs')) {
        throw "CI classification returned an invalid mode: '$mode'."
    }

    $jobs = @('setup', 'clippy', 'test-windows', 'test-ubuntu',
        'setup-integration', 'feature-matrix', 'audit', 'deny')
    if (@($Needs.PSObject.Properties).Count -ne ($jobs.Count + 1)) {
        throw 'CI gate received an unexpected dependency set.'
    }
    $expected = if ($mode -ceq 'full') { 'success' } else { 'skipped' }
    foreach ($job in $jobs) {
        $dependency = $Needs.PSObject.Properties[$job]
        if (-not $dependency -or $dependency.Value.result -cne $expected) {
            throw "CI job '$job' must be '$expected' for mode '$mode'."
        }
    }
}
