---
title: "feat: Release ddev-workspaces v0.1.0 for macOS ARM"
type: feat
date: 2026-08-29
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# Release ddev-workspaces v0.1.0 for macOS ARM

> Historical record: this plan describes the original manual v0.1.0 release
> proposal and is retained for its execution evidence. It is superseded by
> the dist-generated [`release.yml`](../../.github/workflows/release.yml) and
> [`dist-workspace.toml`](../../dist-workspace.toml), which are now the sole
> supported release procedure. All requirements and commands below are a
> historical snapshot, not current instructions. Do not run the manual `gh
> release` sequence in this document; it is not a competing release path.

## Goal Capsule

- **Objective:** A user on the verified Apple Silicon macOS environment with repository access can download the accepted `ddev-workspaces` v0.1.0 CLI, verify its integrity, install it, and complete one safe disposable source-only lifecycle.
- **Means:** Build the unchanged CLI from one clean `origin/main` commit with Cargo's lockfile, package the native executable in one mode-preserving archive, publish it and its SHA-256 checksum through a draft GitHub Release, then verify the downloaded asset before declaring the release complete. (KTD1-KTD4)
- **Authority:** This plan and the current user decisions define release scope; the exact clean `origin/main` commit defines source; `Cargo.lock` defines dependencies; the existing local checks and disposable smoke define acceptance; GitHub's resulting tag, release, and assets define publication state.
- **Execution profile:** Run locally on an Apple Silicon Mac with the repository-pinned Rust toolchain and authenticated GitHub CLI. Hosted CI is not a prerequisite because GitHub Actions billing is unavailable.
- **Stop conditions:** Stop before publication if the checkout is not clean and exactly based on current `origin/main`, accepted Rust inputs differ from `7b0cc0e329a6e723cbae0999c4ea7478dd44777f`, v0.1.0 already exists remotely, local verification fails, or the host is not `aarch64-apple-darwin`. After publication, a downloaded-checksum or disposable-lifecycle failure stops completion and enters the bounded post-publication rollback policy.
- **Tail ownership:** The release executor owns temporary staging, the draft/tag rollback described below, post-download verification, exact removal of the smoke fixture, and a final clean-state receipt. A reviewer must authorize a separate release task before any tag, release, asset, or installation is created.

---

## Product Contract

### Summary

Publish the already-complete CLI as the first GitHub Release, v0.1.0, for macOS ARM. This is a distribution task, not a product-behavior change or broader rollout.

### Problem Frame

The CLI is accepted and `Cargo.toml` already declares 0.1.0, but the repository has no tag or GitHub Release. Users currently have only source-oriented local build instructions, so there is no versioned executable, integrity file, or verified download path.

### Key Decisions

- **Release the accepted CLI without reopening product behavior.** Governs R1-R3 and R10. (session-settled: user-directed — chosen over another feature pass because the immediate goal is distribution of the finished CLI.)
- **Support macOS ARM only in v0.1.0.** Governs R4-R6. (session-settled: user-directed — chosen over a cross-platform matrix because Linux and Windows remain deferred.)
- **Keep release execution local.** Governs R5-R8. (session-settled: user-directed — chosen over hosted CI because GitHub Actions billing is unavailable and must not gate the release.)
- **Use current built-in tools before adding machinery.** Governs R5-R9. (session-settled: user-directed — chosen over release automation, packaging frameworks, installers, signing, package managers, and updaters.)

### Requirements

- R1. Release exactly one clean `origin/main` commit and record its full SHA before building or creating remote state.
- R2. Confirm the release commit changes none of `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `src/`, or `tests/` relative to the accepted source commit `7b0cc0e329a6e723cbae0999c4ea7478dd44777f`; stop for review if this invariant is false.
- R3. Keep the crate at version 0.1.0. Because `src/cli.rs` already exposes `env!("CARGO_PKG_VERSION")` through Clap, make no version-reporting source or test change.
- R4. Produce exactly `ddev-workspaces-v0.1.0-aarch64-apple-darwin.tar.gz`, containing one executable named `ddev-workspaces`, and `ddev-workspaces-v0.1.0-aarch64-apple-darwin.tar.gz.sha256`.
- R5. Build with `cargo build --release --locked` on a local `aarch64-apple-darwin` host after the full existing local verification passes.
- R6. Use SHA-256 over the final archive, preserve the executable bit in the archive, and verify archive contents and checksum before any upload.
- R7. Publish one non-prerelease GitHub Release named `ddev-workspaces v0.1.0` at tag `v0.1.0`, targeted at the recorded release SHA, using authenticated `gh`. Create it as a draft with both assets and concise notes, inspect it, then publish it.
- R8. Download both published assets into a fresh temporary directory, verify the checksum and archive shape independently of the staging copy, extract the executable, and prove its architecture, help, and version output.
- R9. Install only the downloaded executable into a disposable temporary prefix for release verification. In a fresh disposable Git repository, run `doctor`, source-only dry-run, `create --source-only`, `list`, removal dry-run, and confirmed removal; then remove the retained fixture branch and prove the fixture and ownership record are gone.
- R10. Release notes state macOS ARM support, the exact macOS version used for build and verification, runtime prerequisites (`git`, plus `ddev` only for DDEV-configured projects), SHA-256 verification, archive extraction and installation, `--help`/`--version` checks, and the deferred scope without claiming an untested minimum macOS version, signing, or notarization.
- R11. If draft creation, upload, inspection, publication, or post-publication verification is incorrect, follow the bounded rollback policy below; never overwrite a published asset or move an existing release tag.

### Success Criteria

- The published tag resolves to the recorded clean release SHA.
- The release is neither draft nor prerelease and exposes exactly the archive and checksum named in R4.
- Freshly downloaded bytes pass the published SHA-256 check; the extracted executable is ARM64, prints `ddev-workspaces 0.1.0`, renders help, and completes the disposable source-only lifecycle.
- Repository source, manifests, lockfile, tests, workflows, pilot projects, installed system paths, and pre-existing Git/DDEV state remain unchanged.

### Scope Boundaries

#### In v0.1.0

- One native macOS ARM executable archive built and verified on the recorded local macOS version, one checksum, one tag, one GitHub Release, concise notes, download verification, and one safe disposable source-only smoke.
- The exact accepted synchronous CLI and its existing four commands.

#### Deferred to Follow-Up Work

- Permanent Documentably and Posts Table Pro configurations or any broader project rollout.
- Linux, Windows, universal binaries, signing/notarization, package-manager publishing, auto-update, retry/resume, and new CLI features.

#### Outside This Release

- Hosted release CI, release automation frameworks, installer scripts, Homebrew taps, dependency changes, workflow changes, and repetition of the completed acceptance pilots.

### Acceptance Examples

- AE1. Given a clean release checkout whose Rust inputs match the accepted commit, when locked verification and the native release build pass, then staging contains only the exact archive and checksum with an executable ARM64 member.
- AE2. Given a reviewed draft targeted at the recorded SHA, when it is published and both assets are downloaded fresh, then checksum, tag target, release metadata, architecture, help, and version all match this plan.
- AE3. Given the downloaded executable installed under a temporary prefix and a fresh local Git fixture, when the source-only lifecycle runs, then create/list/remove succeed without DDEV, network, project credentials, or retained worktree metadata; the retained fixture branch is then removed explicitly.
- AE4. Given an incorrect draft or a failed post-publication verification, when rollback is authorized by R11, then only `v0.1.0`'s release and tag are removed and all local release temporary paths are deleted; unrelated releases, tags, assets, and repository state are untouched.

---

## Planning Contract

### Key Technical Decisions

- KTD1. **No version-reporting implementation work.** `src/cli.rs` already calls Clap `.version(env!("CARGO_PKG_VERSION"))`, so `ddev-workspaces --version` is derived from the existing `Cargo.toml` value. Adding a flag, constant, dependency, or test would duplicate an exercised framework capability. (session-settled: user-directed — chosen over a conventional but unnecessary CLI change; governs R3.)
- KTD2. **Native locked build from one recorded commit.** Verification and `cargo build --release --locked` run in the same clean checkout on the pinned toolchain; the recorded SHA is passed explicitly to GitHub rather than relying on ambient default-branch timing. (session-settled: user-directed — chosen over hosted CI and cross-platform builds; governs R1, R2, R5, R7.)
- KTD3. **One `tar.gz` plus one SHA-256 file.** The archive preserves the executable mode and stable binary name while remaining buildable with macOS `tar`; `shasum -a 256` supplies the integrity file. No packager or signing layer is needed. This agent-proposed mechanism is justified over a bare executable by mode preservation and over a packaging framework by the verified built-in tools. Governs R4 and R6.
- KTD4. **Draft, inspect, publish, then download.** `gh release create --draft` uploads both assets and creates the exact tag target; `gh release view` checks draft metadata; `gh release edit --draft=false` publishes; `gh release download` provides an independent post-publication copy. This agent-proposed sequencing provides the required unpublished-state rollback point without adding automation. Governs R7-R11.
- KTD5. **One disposable source-only smoke.** The installed downloaded binary uses a temporary local Git repository and no `[ddev]` configuration. This proves the distributed executable's safe lifecycle without repeating either accepted product pilot. (session-settled: user-directed — chosen over rerunning the two full pilots; governs R8-R10.)

### Historical Manual Release Sequence and Command Contract (Superseded)

Run from a fresh local checkout after the plan PR is reviewed and merged. Use task-specific variables such as `release_sha`, `release_stage`, and `release_download`; do not place secrets in scripts or notes.

1. Fetch `origin/main`, detach at it, require an empty `git status --porcelain`, record `release_sha=$(git rev-parse HEAD)`, and require `git rev-parse origin/main` to equal it. Require `git diff --exit-code 7b0cc0e329a6e723cbae0999c4ea7478dd44777f "$release_sha" -- Cargo.toml Cargo.lock rust-toolchain.toml src tests` to pass.
2. Require `uname -m` to print `arm64`, `rustc -vV` to report `host: aarch64-apple-darwin`, `cargo metadata --locked --format-version 1 --no-deps` to report package version `0.1.0`, and `gh auth status` to pass. Reconfirm with `gh repo view alessandrotesoro/ddev-workspaces --json defaultBranchRef` that `main` is the default branch and with `gh api repos/alessandrotesoro/ddev-workspaces/immutable-releases --jq .enabled` whether release immutability is enabled. Require `git ls-remote --exit-code --tags origin refs/tags/v0.1.0` and `gh release view v0.1.0 --repo alessandrotesoro/ddev-workspaces` both to report absence. Treat any existing tag or release as a hard stop, not a replaceable target.
3. Run the complete local gate: `cargo fmt --check`; `cargo clippy --all-targets --all-features --locked -- -D warnings`; `cargo test --all-targets --all-features --locked`; `cargo build --release --locked`.
4. Create `release_stage` with `mktemp -d`, register cleanup for that exact resolved directory, create its explicit child `"$release_stage/payload"`, copy `target/release/ddev-workspaces` there as `ddev-workspaces`, set mode `0755`, and require its `--version` output to equal `ddev-workspaces 0.1.0` and `--help` to succeed.
5. Create `ddev-workspaces-v0.1.0-aarch64-apple-darwin.tar.gz` in `release_stage` from `"$release_stage/payload/ddev-workspaces"`; run `tar -tzf` and `tar -tvzf` to require exactly one `ddev-workspaces` member with executable mode; run `file` and `lipo -archs` on the payload binary to require ARM64; create the sibling `.sha256` with `(cd "$release_stage" && shasum -a 256 "$release_asset" > "$release_checksum")`; verify it with `(cd "$release_stage" && shasum -a 256 --check "$release_checksum")`. Delete the exact payload binary with `unlink`, remove its now-empty parent with `rmdir`, and require that `release_stage` contains only the two exact R4 filenames before upload.
6. Write the following concise notes to a separate `release_notes` temporary file outside `release_stage`, substituting the exact `sw_vers -productVersion` observed during execution and no credentials or machine paths:

   ~~~~markdown
   First release of `ddev-workspaces`, a conservative local Git worktree and DDEV workspace manager.

   Supported artifact: macOS ARM64 (`aarch64-apple-darwin`), built and verified on macOS `<recorded-version>`. This release does not claim qualification on an earlier minimum macOS version. Runtime requires `git`; repositories with a `[ddev]` configuration also require `ddev`.

   Download the `.tar.gz` and `.sha256` assets, place them in the same directory, then verify and install:

   ```sh
   shasum -a 256 --check ddev-workspaces-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
   tar -xzf ddev-workspaces-v0.1.0-aarch64-apple-darwin.tar.gz
   sudo mkdir -p /usr/local/bin
   sudo install -m 0755 ddev-workspaces /usr/local/bin/ddev-workspaces
   ddev-workspaces --version
   ddev-workspaces --help
   ```

   This release is not signed or notarized. Linux, Windows, universal binaries, package-manager distribution, auto-update, retry/resume, and permanent pilot-project configuration are not included.
   ~~~~

7. Create the draft with `gh release create v0.1.0 "$release_stage/$release_asset" "$release_stage/$release_checksum" --repo alessandrotesoro/ddev-workspaces --target "$release_sha" --title "ddev-workspaces v0.1.0" --notes-file "$release_notes" --draft`. Inspect `gh release view v0.1.0 --repo alessandrotesoro/ddev-workspaces --json tagName,targetCommitish,name,isDraft,isPrerelease,isImmutable,assets,url`; require the exact tag, SHA target, title, draft state, non-prerelease state, and exactly the two R4 asset names. Publish with `gh release edit v0.1.0 --repo alessandrotesoro/ddev-workspaces --draft=false --latest`.
8. Create a separate `release_download` directory with `mktemp -d`, download only the two exact assets with `gh release download v0.1.0 --repo alessandrotesoro/ddev-workspaces --dir "$release_download" --pattern 'ddev-workspaces-v0.1.0-aarch64-apple-darwin.tar.gz*'`, and repeat checksum, one-member archive, extraction, `file`, `lipo -archs`, `--version`, and `--help` checks against the downloaded bytes.
9. Install the extracted downloaded binary only to `"$release_download/install/bin/ddev-workspaces"`. Create the disposable Git repository at the explicit child `"$release_download/smoke-repository"`, configure local fixture identity, track `.gitignore`, `.ddev-workspaces.toml`, and a small source file, and commit them. The config uses `version = 1`, `project_id = "release-smoke"`, `workspace_root = ".worktrees"`, with `.worktrees/` ignored and no DDEV/files/commands/checks. Run installed-binary `doctor`, `create --source-only --dry-run --base HEAD release-smoke`, `create --source-only --base HEAD release-smoke`, `list`, `remove --dry-run release-smoke`, and `remove --confirm release-smoke release-smoke`. Require the worktree and ownership record to be absent, delete the retained `release-smoke` branch, and require `git status --porcelain` to be empty.
10. Re-read the published release JSON and remote tag target, require `gh api repos/alessandrotesoro/ddev-workspaces/releases/latest --jq .tag_name` to equal `v0.1.0`, and record the release URL and `release_sha`. Delete the validated `release_stage` and `release_download` directories and the separate `release_notes` file, prove all three are absent, and require the release checkout to remain clean.

### Rollback Policy

- **Before draft creation:** delete only the validated temporary staging/download directories and release-notes file. No remote rollback is needed.
- **Incorrect or incomplete draft:** inspect it first with `gh release view`; then run `gh release delete v0.1.0 --repo alessandrotesoro/ddev-workspaces --cleanup-tag --yes`. Confirm both release and tag are absent before correcting local inputs. Never reuse unverified staged bytes.
- **Incorrect published release detected by the immediate verification in this plan:** do not edit assets, overwrite the tag, or announce the release. Record the failed check and exact release URL/SHA, then use the same bounded `gh release delete ... --cleanup-tag --yes` command only if repository policy allows deletion. Confirm absence before a separately authorized retry. If immutable releases become enabled or deletion is refused, stop and escalate; do not attempt to bypass immutability.
- **Correct published release:** the tag and assets are immutable release records for this plan. Subsequent fixes require a new version, not replacement of v0.1.0.

### Risks and Dependencies

- GitHub state can change between preflight and publication. Passing `--target "$release_sha"` and re-reading the remote tag after publication prevents an ambient `main` update from changing the released commit.
- A macOS binary may trigger local Gatekeeper warnings because signing and notarization are excluded. Release notes state that limitation without adding an unsupported workaround.
- `gh release create` performs separate draft, upload, and publication operations internally. Explicit draft mode plus bounded cleanup handles partial remote state while preserving unrelated tags and releases.
- The GitHub CLI version and repository settings are execution-time dependencies. Recheck authentication, release/tag absence, default branch, and immutable-release status immediately before mutation.

### Sources and Research

- `Cargo.toml`, `Cargo.lock`, and `rust-toolchain.toml` define version 0.1.0, locked dependencies, Rust 1.97.1, and an `aarch64-apple-darwin` local host.
- `src/cli.rs` proves Clap already owns `--version`; `tests/` and `ddev-workspaces-plan.md` define the existing local verification and disposable fixture conventions.
- `README.md` records the four-command surface, runtime prerequisites, and successful Documentably/Posts Table Pro pilot evidence from 2026-08-29.
- The local planning baseline is macOS 26.5.2 on ARM64 with Cargo/Rust 1.97.1 and GitHub CLI 2.96.0. The repository is private, uses `main` as its default branch, has release immutability disabled, and had no tags or releases when this plan was grounded; every mutable fact is rechecked before release mutation.
- `$codex-evidence-gates:evaluate-cargo-alternatives` returned `USE_EXISTING`: Cargo's locked build, existing Clap version binding, macOS archive/checksum tools, and GitHub CLI cover the complete release contract. Its caller-shaped check produced `ddev-workspaces 0.1.0`, verified the exact archive/checksum shape, and cleaned its disposable directory; no crate search, trial, adoption, or repository edit was justified.
- [GitHub release management](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository) documents tags, draft releases, assets, publication, deletion, and immutable-release considerations.
- [GitHub CLI release creation](https://cli.github.com/manual/gh_release_create), [download](https://cli.github.com/manual/gh_release_download), [view](https://cli.github.com/manual/gh_release_view), [edit](https://cli.github.com/manual/gh_release_edit), and [delete](https://cli.github.com/manual/gh_release_delete) define the exact local capabilities used by KTD4 and rollback.

---

## Implementation Units

### U1. Verify and stage the macOS ARM release

- **Goal:** Produce locally verified v0.1.0 release assets from the exact accepted CLI source without changing the repository.
- **Requirements:** R1-R6, R10; AE1; KTD1-KTD3.
- **Dependencies:** Reviewer authorization and a clean `origin/main` containing only the approved plan changes after `7b0cc0e329a6e723cbae0999c4ea7478dd44777f`.
- **Files:** No repository files. `release_stage` contains only the archive and checksum; the release-notes file is a separate temporary path.
- **Approach:** Execute sequence steps 1-6. Stop on any source, version, host, verification, architecture, archive, or checksum mismatch.
- **Execution note:** This is packaging work; use the existing locked test suite and deterministic artifact inspection, not new unit tests or coverage tooling.
- **Patterns to follow:** Existing Cargo verification in `ddev-workspaces-plan.md`; version binding in `src/cli.rs`; temporary-directory discipline in `tests/support/mod.rs`.
- **Test expectation:** None — the unit changes no behavior or repository code; its proof is the existing suite plus artifact checks.
- **Verification:** The checkout remains clean and staging contains exactly the two R4 assets, both derived from an ARM64 executable that reports 0.1.0.

### U2. Publish, download, and smoke-test v0.1.0

- **Goal:** Publish the exact staged assets at the exact recorded commit and prove the distribution path using freshly downloaded bytes.
- **Requirements:** R7-R11; AE2-AE4; KTD4-KTD5.
- **Dependencies:** U1.
- **Files:** No repository files. Draft creation adds tag `v0.1.0`, one draft release, and two assets; publication occurs only after that state passes inspection. Temporary verification uses a disposable install prefix and a Git fixture nested under the download directory.
- **Approach:** Execute sequence steps 7-10. Use the Rollback Policy for partial or incorrect state and stop rather than overwriting any remote object.
- **Execution note:** Keep the smoke synchronous and source-only; it must not invoke DDEV, Docker, network-dependent project setup, or either accepted pilot.
- **Patterns to follow:** Source-only lifecycle and cleanup behavior in `README.md` and `tests/lifecycle.rs`; exact remote inspection through GitHub CLI JSON.
- **Test expectation:** None — the unit changes no product code; AE2 and AE3 are release-boundary smoke checks.
- **Verification:** The final release JSON and remote tag match the recorded SHA, fresh downloads pass every integrity/runtime check, the disposable lifecycle leaves no worktree/record/branch, temporary directories are removed, and the release checkout is clean.

---

## Verification Contract

| Verification | Done signal |
|---|---|
| Source provenance | Clean `origin/main`; accepted Rust inputs unchanged since `7b0cc0e329a6e723cbae0999c4ea7478dd44777f`; recorded full release SHA. |
| Rust quality gate | Format, Clippy with warnings denied, all targets/features tests, and locked release build pass on the pinned local toolchain. |
| Local artifact gate | Exact two filenames; archive has one executable member; staged binary is ARM64 and reports 0.1.0; checksum verifies. |
| GitHub publication gate | Tag resolves to the recorded SHA; release is published, latest, non-prerelease, and has exactly the two named assets. |
| Independent download gate | Fresh downloads pass checksum, archive, architecture, version, and help checks. |
| Installed-binary smoke | Temporary-prefix binary completes doctor, source-only dry-run/create/list/remove and exact fixture cleanup without DDEV or retained state. |
| Cleanup gate | Temporary staging/download/fixture paths are gone; repository checkout is clean; no unrelated local or remote state changed. |

---

## Definition of Done

- R1-R11 and AE1-AE4 are satisfied in order with a recorded command receipt and no skipped gate.
- GitHub exposes one `v0.1.0` release at the exact recorded clean `origin/main` SHA with the exact archive and checksum.
- A fresh download independently verifies and the installed downloaded binary passes help, version, architecture, and one disposable source-only lifecycle.
- Release notes provide accurate macOS ARM installation and verification instructions, name the exact build/verification macOS version without claiming a lower compatibility floor, and state the unsigned/not-notarized limitation and deferred platforms/features.
- No source, test, dependency, lockfile, workflow, permanent project configuration, system installation, or unrelated documentation change is made by the release task.
- Documentably and Posts Table Pro acceptance evidence is reused, not rerun, and no pilot resource is created.
- Temporary files and the smoke fixture are removed, the release checkout is clean, and no abandoned draft, tag, asset, worktree, branch, ownership record, install prefix, or experimental release machinery remains.
- If rollback was required, v0.1.0 is absent remotely and the failure receipt is preserved for a separately authorized retry; otherwise the release URL and SHA are handed back for confirmation.
