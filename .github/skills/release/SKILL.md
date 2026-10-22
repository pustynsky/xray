---
name: release
description: "Publish or repair an xray release correctly: version bump, clean tagged artifact, complete GitHub release notes, xray.exe upload, and post-publish verification."
disable-model-invocation: true
argument-hint: "Target version/tag, e.g. 0.2.2 or v0.2.2"
---
# Release Xray

You are publishing or repairing an `xray` GitHub Release.

This workflow is self-contained. Personal knowledge-base notes are supplemental,
not a required dependency or permission to change the publish boundaries below.

## Workflow

1. Determine the target version from the prompt argument. If it is missing or ambiguous, ask for it.
2. Determine the previous public release tag and compare range, for example `v0.2.1...v0.2.2`.
3. Inspect repository state before making or retrying changes:

   ```powershell
   git --no-pager status --short --branch
   git --no-pager log -1 --oneline --decorate
   git tag --list vNEXT
   git ls-remote origin refs/tags/vNEXT
   gh release view vNEXT --json tagName,name,assets,url,publishedAt
   ```

   Inspect command exit codes. Distinguish a confirmed missing tag/release from
   authentication, network, or permission failures; surface errors before proceeding.

4. If a previous release attempt partially completed, continue from the observed state. Do not blindly rerun commit/tag/release commands. Verify that existing local/remote tags identify the intended commit; do not move or replace a published tag.
5. Leave unrelated dirty or untracked files alone.
6. Bump the package version in both `Cargo.toml` and `Cargo.lock`. Update release documentation only where needed; do not stage unrelated changes or update unrelated dependencies.
7. Build and verify locally, then stage only the release files:

   ```powershell
   cargo build --release
   .\target\release\xray.exe --version
   git diff --cached --check
   ```

8. Before any external publish step (`git push`, tag push, release creation/edit,
   asset upload/replacement), summarize the exact commit/tag/release actions and
   ask the user for approval. Do not commit unrelated staged content. Unless the
   user explicitly opts out, end each created commit message with:

   ```text
   Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
   ```

9. After approval, commit and tag as needed. Build the upload artifact from the
   tagged commit in a unique clean temporary worktree, using its own target
   directory, not the original checkout's binary. Confirm the worktree is clean
   before building and check `xray.exe --version`: it must report the target
   version and tagged commit SHA, with no `-dirty` suffix. Record the tag's full
   commit SHA, the exact artifact path, and its SHA256 using `Get-FileHash`.
   Keep that file unchanged for publication. Stop on any build or identity failure;
   do not publish until the release body is ready in step 11.
10. Prepare release notes locally for the full range, not just the top
    `CHANGELOG.md` section:

    ```powershell
    git --no-pager log --oneline --reverse vPREV..vNEXT
    ```

    Use the local commit/PR history and every relevant changelog section in that
    range. For a first release, use history through `vNEXT` with no previous-tag
    range. GitHub's notes generator is optional: call it only if the remote tag
    already exists and identifies the recorded release commit. A fresh local tag
    is not available to that API before step 12; do not push early just to generate
    notes. When using the generator, merge its output with the local history and
    omit the previous-tag parameter for a first release.

11. Ensure the release body includes full highlights, all meaningful PRs/direct
    commits, the full compare link (or tag/history link for a first release),
    `xray.exe`, and the recorded clean-build SHA256. Interpolate variables before
    publishing; no placeholders such as `$hash`, `TODO`, `vNEXT`, or `vPREV`.
12. Within the approved action set, push the commit/tag and create the GitHub
    release with the final body and that exact artifact path. For an existing
    release, edit its body and upload/replace assets only as approved. Check each
    command's exit code; on a failure or timeout, inspect remote state before
    retrying. Never upload to a nonexistent release or publish placeholder notes
    to create an upload target.
13. Read back the release metadata/body and download the published `xray.exe` to
    a separate unique temporary directory. Before running it, require its SHA256
    to equal the recorded clean-build hash and the release-notes hash; also compare
    GitHub's asset digest when available. A missing optional digest is not proof
    of a match. Then run `--version` and require the target version, tagged commit
    SHA, and no `-dirty` suffix. Any mismatch is a failed release verification.
14. For an asset repair, establish the same clean tagged-build identity before
    replacing it and obtain approval for the replacement. A notes-only repair
    does not authorize changing tags or assets. If the original clean-build hash
    is unavailable, say that binary provenance remains unverified; do not claim
    the existing download is identical to a new build unless hashes actually match.
15. Remove only the specific temporary worktree/directories created for this run,
    after verifying their resolved paths and containment. Use Git's worktree removal
    for the worktree; do not force cleanup of unexpected changes. Report retained
    artifacts when cleanup cannot be completed safely.

## Output

When done, report:

- Release URL
- Commit SHA and tag
- Asset name and SHA256
- Clean-build and downloaded SHA256 comparison, version/commit identity, and any
  unavailable optional asset digest
- Any unrelated dirty files left untouched
