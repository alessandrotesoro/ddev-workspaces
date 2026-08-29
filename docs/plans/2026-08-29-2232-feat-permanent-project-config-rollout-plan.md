---
title: "Permanent Project Configuration Rollout - Plan"
type: feat
date: 2026-08-29
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# Permanent Project Configuration Rollout - Plan

## Goal Capsule

- **Objective:** Documentably and Posts Table Pro contributors can create clean, project-prepared workspaces from repository-owned configuration without reconstructing temporary pilot inputs or changing the `ddev-workspaces` binary.
- **Means:** Land one minimal, independent configuration PR in each target repository, prove each PR from its exact pushed commit in a disposable clone, and roll out Documentably before Posts Table Pro. (KTD1-KTD4)
- **Authority:** This plan defines rollout scope; each target repository's current default branch, instructions, manifests, ignore rules, and tracked files define its permanent settings; `ddev-workspaces` v0.1.0 defines configuration and lifecycle behavior.
- **Execution profile:** After this plan is authorized and merged, use one separate implementation task and branch per target repository. Use the published v0.1.0 macOS ARM binary for disposable acceptance.
- **Stop conditions:** Stop a target PR if its default branch, required command, tracked template, ignore boundary, readiness artifact, or standalone DDEV behavior no longer matches this plan. Stop rather than adding CLI behavior, secrets, machine paths, compatibility, retries, or target-specific abstraction.
- **Tail ownership:** Each target implementation task owns its disposable clone, workspace, retained branch, DDEV identity, container, network, image, and evidence receipt until exact cleanup is proven. No task may modify the normal target checkout or another repository.

---

## Product Contract

### Summary

Add permanent repository-owned `ddev-workspaces` configuration for the two proven targets. Keep the rollout to small independent target-repository diffs and reuse the shipped v1 schema without changing the CLI.

### Problem Frame

The disposable Documentably and Posts Table Pro pilots passed, but their untracked configuration and local inputs were removed by design. Contributors therefore cannot reproduce the accepted workflows from either repository alone. The release plan explicitly deferred permanent target configuration to follow-up work.

### Key Decisions

- **Keep configuration in each target repository.** This is the existing v1 ownership boundary and avoids hard-coded project knowledge in the binary. Governs R1-R5.
- **Roll out only the two proven targets.** Additional repositories remain outside this unit until their real workspace and runtime boundaries have their own evidence. Governs R6-R9.
- **Use the shipped schema unchanged.** Current parser, preparation, DDEV, and readiness behavior cover both target workflows. Governs R10-R12.

### Requirements

**Shared configuration and safety**

- R1. Each target repository must track its own `.ddev-workspaces.toml` with `version = 1`, a target-specific DNS-safe `project_id`, and an already-ignored `.worktrees` root.
- R2. Each target diff must contain no secret value, absolute user path, disposable identity, generated DDEV state, local pilot input, or machine-specific state.
- R3. Each target configuration must declare only commands, files, and checks required by its verified current workflow.
- R4. Acceptance must use a fresh disposable clone and `--base` set to the full pushed target-config commit so the proof exercises the proposed configuration rather than the ambient default branch.
- R5. Managed removal must be followed by deletion of the retained disposable workspace branch and proof that all exact temporary resources are absent.

**Documentably**

- R6. Documentably must use `project_id = "documentably"`, `workspace_root = ".worktrees"`, no `[ddev]` section, no file rule, and no secret-bearing input.
- R7. Documentably normal creation must run `corepack pnpm install --frozen-lockfile` and then `corepack pnpm build` from the repository root.
- R8. Documentably readiness must require `packages/cli/dist/index.js`; all generated dependency, Turbo, and `dist` output must remain ignored so the managed worktree stays clean.

**Posts Table Pro**

- R9. Posts Table Pro must use `project_id = "posts-table-pro"`, `workspace_root = ".worktrees"`, `[ddev].app_root = "."`, one tracked safe DDEV template, and one file rule that publishes that template as ignored `.ddev/config.yaml`.
- R10. The Posts Table Pro template must set WordPress classification, `docroot: .`, `disable_settings_management: true`, `performance_mode: none`, and `omit_containers: [db]`; it must omit a fixed project name, uploads, certificates, snapshots, imports, environment secrets, and database configuration.
- R11. Posts Table Pro must declare no package command and no explicit check: tracked-source verification, file-rule readiness, and exact DDEV name/path/running/Mutagen readiness already own the accepted contract.
- R12. Posts Table Pro implementation and acceptance must use a clean standalone checkout outside the existing parent Barn2 DDEV application and must not touch or reset the dirty normal checkout.

**DDEV Workspaces**

- R13. DDEV Workspaces must receive no source, test, manifest, lockfile, release, or dependency change for this rollout.
- R14. The only DDEV Workspaces repository changes are this plan and its README plan-index link.

### Success Criteria

- Each target PR passes `doctor`, exact-base dry-run, full creation, `list`, managed-workspace `doctor`, removal dry-run, confirmed removal, retained-branch deletion, and exact cleanup from its pushed commit.
- Documentably finishes with the pinned install and build complete, the declared CLI artifact present, and a clean managed worktree.
- Posts Table Pro finishes with one exact running web runtime, no database container, Mutagen disabled, a clean managed worktree, and no change to the parent Barn2 DDEV application or normal checkout.
- The DDEV Workspaces repository remains behaviorally identical to v0.1.0.

### Acceptance Examples

- AE1. Given the pushed Documentably configuration commit in a fresh clone, when normal `create --base <full-sha>` runs, then the pinned install and build complete, `packages/cli/dist/index.js` exists, and the workspace reports ready without DDEV.
- AE2. Given the pushed Posts Table Pro configuration commit in a fresh standalone clone, when normal `create --base <full-sha>` runs, then the template is copied privately to `.ddev/config.yaml` and the exact generated DDEV identity is running with one web runtime, no database, and disabled Mutagen.
- AE3. Given either accepted disposable workspace, when dry-run and confirmed removal complete, then the workspace and ownership record are absent; after explicit branch and fixture cleanup, no target-specific temporary resource remains.

### Scope Boundaries

#### In This Rollout

- One configuration PR for Documentably and one for Posts Table Pro.
- One disposable exact-commit acceptance receipt per target.
- The plan and one README index link in DDEV Workspaces.

#### Deferred to Follow-Up Work

- Document Library Advanced: real linked release worktrees exist, but its current workflow spans a shared parent DDEV site and `wp-env` with database/upload mappings. A separate decision and pilot must establish a standalone boundary before permanent configuration.
- `app.filebean`: real linked worktrees and tracked DDEV configuration exist, but the current multi-application setup also owns tunnel, worker, Shopify, Composer, and pnpm preparation. It needs a separately scoped minimal contract.

#### Rejected for This Rollout

- `customers`: a local DDEV application exists, but it is not a Git repository and therefore cannot be a `ddev-workspaces` target.
- `filebean-app`: a Git repository and DDEV configuration exist, but no current linked-worktree workflow was verified.
- Any central registry, generator, shared schema package, installer, orchestration service, adapter layer, or rollout framework.
- Installer scripts, retry/resume, auto-update, new commands, compatibility readers, release automation, dependency changes, and unrelated product work.

### Grounded Target-Project Table

| Target | Disposition | Current proof | Permanent minimal shape |
|---|---|---|---|
| Documentably | Include first | Remote `main` advertised `153fa27ddfdc9a32213e2e083a6ba2d5e0d7d464`; `packageManager` pins pnpm 10.34.5; root build delegates to Turbo; the successful pilot proved the same install/build pair and ignored CLI artifact | `.ddev-workspaces.toml` plus `/.worktrees/` ignore; two root commands and one artifact check; no DDEV or files |
| Posts Table Pro | Include second | Remote `master` advertised `6ed8023b236ac3819060760036dfc5c45e19359c`; tracked plugin entrypoint and vendored runtime passed the isolated DDEV pilot; the normal checkout is dirty and nested in another DDEV app | `.ddev-workspaces.toml`, `.ddev-workspaces.ddev.yaml`, and ignores for `/.worktrees/` and `/.ddev/`; one template file rule; no commands or explicit checks |
| Document Library Advanced | Defer | Linked release worktrees are present, but standalone DDEV versus shared-site/`wp-env` ownership is unresolved | Separate configuration decision and pilot |
| `app.filebean` | Defer | Tracked DDEV config and several linked worktrees are present; setup spans pnpm, DDEV Composer, worker, tunnel, and Shopify workflows | Separate minimal-setup analysis |
| `customers` | Reject | Local DDEV config exists without a Git repository | Not eligible |
| `filebean-app` | Reject for now | Git and tracked DDEV config exist, but no linked-worktree workflow was verified | Reassess only after a real worktree need exists |

---

## Planning Contract

### Key Technical Decisions

- KTD1. **Use independent inline project configurations.** The v1 schema is already the shared contract. Two target files with different responsibilities are cheaper and safer than a registry, generator, adapter, or shared package. Governs R1-R3 and R13.
- KTD2. **Automate Documentably's proven host preparation.** The permanent non-DDEV configuration converts the pilot's manual pinned install/build into ordered shell-free commands and keeps the existing artifact check. Governs R6-R8.
- KTD3. **Publish a safe Posts Table Pro DDEV template.** A tracked root template is copied into the ignored `.ddev/` directory for each workspace. This preserves repository ownership without importing the parent Barn2 application or committing generated DDEV state. Governs R9-R12.
- KTD4. **Use sequential exact-commit rollout.** Merge the plan first, then prove and merge Documentably before starting Posts Table Pro acceptance. The simpler non-DDEV path validates permanent command orchestration before the DDEV path adds runtime state. Governs R4-R5.
- KTD5. **Do not add Rust or async work.** The current strict parser, shell-free command runner, file publication, readiness checks, and exact DDEV lifecycle already cover the rollout. The Rust best-practices Rule of Three also rejects a new abstraction for two independent configurations. Governs R13-R14.

### Assumptions

- The observed target default-branch SHAs are evidence anchors, not frozen implementation bases. Each implementation task must refresh the remote default branch and reverify every named field before editing.
- The published v0.1.0 macOS ARM binary remains available to the implementation tasks.
- Posts Table Pro's standalone DDEV proof is a workspace/runtime smoke, not a functional WordPress or database test.

### Branch and PR Order

1. Merge the DDEV Workspaces plan PR from `plan/permanent-project-config-rollout` into `main` without implementation changes.
2. Create Documentably branch `feat/ddev-workspaces-config` from refreshed `origin/main`. Open, prove, review, and merge that PR.
3. Create Posts Table Pro branch `feat/ddev-workspaces-config` from clean refreshed `origin/master` in an isolated checkout. Open, prove, review, and merge that PR only after Documentably succeeds.
4. Do not open an additional-candidate PR in this unit.

### Evidence-Gate Ledger

| Gate | Disposition | Evidence |
|---|---|---|
| `compound-engineering:ce-plan` | Applied | Produced this implementation-ready planning contract from current repository evidence |
| `rust-best-practices` | Applied | Existing direct schema and two independent configs avoid a premature abstraction |
| `rust-testing` | Applied | Existing behavioral suites already cover config, preparation, DDEV, and lifecycle; target smoke proof is the proportionate test |
| `rust-async-patterns` | Applied | No concurrent or asynchronous mechanism is present or justified |
| `codex-evidence-gates:ground-plan` | Applied — `PLAN GROUNDED` | Reconciled every requirement and proof method against the current schema, lifecycle, target manifests, ignore rules, tracked files, and live workflow boundaries |
| `codex-evidence-gates:audit-overengineering` | Required after grounding | Must disposition every plan mechanism and proof method read-only |
| `codex-evidence-gates:evaluate-cargo-alternatives` | NOT_APPLICABLE | No Rust symbol, dependency, manifest, lockfile, or CLI behavior changes |

---

## Implementation Units

### U1. Documentably permanent preparation contract

- **Goal:** Make one Documentably workspace fully prepared and ready through the repository-owned non-DDEV configuration.
- **Requirements:** R1-R8, R13; Covers AE1 and AE3.
- **Dependencies:** The plan PR is authorized and merged.
- **Files:** Documentably `.ddev-workspaces.toml`; Documentably `.gitignore`.
- **Approach:** Add the exact R6-R8 configuration and one root-anchored `/.worktrees/` ignore. Do not add a DDEV section, file rule, secret input, wrapper script, or README change.
- **Execution note:** This is configuration work; prove it through an exact-commit disposable full-create smoke rather than adding application unit tests.
- **Patterns to follow:** Root `package.json` for the pinned package manager and build script; `.gitignore` for generated-output ownership; `packages/cli/package.json` for the readiness artifact.
- **Test scenarios:**
  - Covers AE1. In a fresh clone at the pushed configuration commit, `doctor` and exact-base dry-run succeed without mutation, then full creation runs the install before the build and reports ready with the CLI artifact present.
  - With generated `node_modules`, Turbo, and package `dist` output present, `list` and managed-workspace `doctor` still report ready and Git reports no tracked or untracked workspace change.
  - Covers AE3. Removal dry-run and confirmed removal delete only the owned workspace and record; explicit retained-branch and clone cleanup leave no disposable path or metadata.
- **Verification:** Record the target SHA, command labels and order, artifact path, clean-worktree result, lifecycle outputs, and cleanup absence checks in the PR.

### U2. Posts Table Pro permanent standalone DDEV contract

- **Goal:** Make one Posts Table Pro workspace start and remove an isolated minimal DDEV runtime from repository-owned safe configuration.
- **Requirements:** R1-R5, R9-R13; Covers AE2 and AE3.
- **Dependencies:** U1 is merged and its acceptance receipt passes.
- **Files:** Posts Table Pro `.ddev-workspaces.toml`; Posts Table Pro `.ddev-workspaces.ddev.yaml`; Posts Table Pro `.gitignore`.
- **Approach:** Add the exact R9-R11 configuration. Ignore `/.worktrees/` and the complete generated `/.ddev/` directory because `.ddev/config.yaml` is published from the tracked root template. Keep the template independent of the parent Barn2 site's DDEV config, database, uploads, certificates, snapshots, generated settings, and secrets.
- **Execution note:** Perform all work from a clean standalone checkout outside the parent Barn2 DDEV application. Do not reset, clean, or edit the existing dirty checkout.
- **Patterns to follow:** The accepted Posts Table Pro pilot values in DDEV Workspaces `README.md` and `ddev-workspaces-plan.md`; the target's tracked plugin entrypoint and vendored runtime boundary.
- **Test scenarios:**
  - Covers AE2. In a fresh standalone clone at the pushed configuration commit, `doctor` and exact-base dry-run succeed without mutation, then full creation copies a byte-identical private DDEV config and reports the exact generated name/path as running.
  - The created runtime has one web container, no database container or project database volume, Mutagen disabled, and no upload, certificate, snapshot, import, secret, or package-command input.
  - The generated `.ddev/` state remains ignored, the tracked template remains unchanged, and `list` plus managed-workspace `doctor` report ready with a clean worktree.
  - Covers AE3. Removal dry-run and confirmed default removal stop and unlist only the exact owned DDEV name; explicit retained-branch, clone, and exact unreferenced image cleanup leave the parent registry, containers, and shared network unchanged.
- **Verification:** Record the target SHA, template hash equality, effective DDEV identity/status, container and volume absence, clean-worktree result, lifecycle outputs, parent-state before/after hashes, and exact cleanup absence checks in the PR.

---

## Verification Contract

| Verification | Applies to | Done signal |
|---|---|---|
| Target `git status --porcelain` and full SHA receipt | U1, U2 | Work starts from a clean refreshed default branch; acceptance uses the exact pushed config commit |
| Published binary `doctor` | U1, U2 | Strict config, ignored workspace root, tracked templates, and target paths are ready without mutation |
| Published binary exact-base full-create dry-run | U1, U2 | Planned branch, path, files, commands, checks, and DDEV identity are correct; no state is created |
| Published binary exact-base full create | U1 | Install, build, artifact readiness, source integrity, and clean-worktree checks pass without DDEV |
| Published binary exact-base full create plus DDEV inspection | U2 | Template publication and exact standalone DDEV readiness pass with no database or Mutagen |
| Published binary `list` and managed-workspace `doctor` | U1, U2 | Readiness recomputes from current source/runtime state |
| Removal dry-run, confirmed default removal, and explicit branch cleanup | U1, U2 | Owned state is removed and no disposable branch, clone, workspace, record, or runtime resource remains |
| DDEV Workspaces diff review | R13, R14 | Only this plan and its README index link differ from the baseline |

No new Cargo test is required. Existing `tests/config.rs`, `tests/preparation.rs`, `tests/ddev.rs`, and `tests/lifecycle.rs` already cover the unchanged mechanisms, and the target-specific acceptance exercises the real repository boundaries that fixtures cannot establish.

---

## Risks and Dependencies

- Target branches can advance after planning. Each implementation task must refresh its target remote and reverify instructions, commands, paths, ignore rules, and secret boundaries before writing.
- Documentably installation requires its declared Node range, Corepack, and network access to locked packages. A failure is a target setup failure, not justification for retry machinery in the CLI.
- Posts Table Pro's normal checkout contains a pre-existing `composer.lock` change and sits inside another DDEV application. Isolation is mandatory to avoid conflating or damaging that state.
- DDEV-generated files evolve. Ignoring the complete runtime `.ddev/` destination prevents generated state from making the owned worktree dirty while the tracked root template remains reviewable.

### Sources and Research

- DDEV Workspaces `README.md`, `src/config.rs`, `src/workspace.rs`, `src/ddev.rs`, and the existing integration tests define the complete v1 configuration, preparation, readiness, and cleanup boundary.
- `ddev-workspaces-plan.md` and the merged v0.1.0 release plan establish the successful pilots, deferred permanent rollout, and no-expansion constraints.
- Documentably `AGENTS.md`, `package.json`, `pnpm-lock.yaml`, `.gitignore`, and `packages/cli/package.json` verify the permanent command and readiness values. Its advertised remote head advanced beyond the original pilot SHA, so implementation must re-ground before editing.
- Posts Table Pro `composer.json`, `package.json`, `.gitignore`, tracked plugin/vendor files, the containing Barn2 DDEV setup, and the accepted pilot verify the standalone template and exclusion boundary. No target `AGENTS.md` exists.
- No DDEV Workspaces `AGENTS.md`, `CONCEPTS.md`, `STRATEGY.md`, or `docs/solutions/` corpus exists at the planning baseline; README, source, tests, and merged plans are the available repository authorities.
- External web research was not load-bearing because the shipped local schema and two accepted pilots directly own this rollout.

---

## Definition of Done

- The DDEV Workspaces plan PR is merged with only this plan and its README index link.
- U1 and U2 land as separate target-repository PRs in the stated order from clean refreshed default branches.
- Each target configuration is minimal, independent, tracked, strict-v1-valid, secret-free, and machine-path-free.
- Every target acceptance scenario passes from the exact pushed config commit and its complete evidence receipt is attached to the owning PR.
- Documentably reaches non-DDEV readiness after its pinned install and build with a clean managed worktree.
- Posts Table Pro reaches exact standalone DDEV readiness without database, Mutagen, parent-site state, package commands, or secrets, and its managed worktree remains clean.
- Removal and explicit cleanup leave no disposable branch, clone, workspace, ownership record, DDEV registration, container, network, volume, or unreferenced exact image.
- The normal target checkouts, parent Barn2 application, DDEV Workspaces source/tests/dependencies, and all unrelated repositories remain unchanged.
- No abandoned pilot file, temporary input, generated state, absolute path, secret, compatibility layer, retry/resume mechanism, installer, registry, generator, adapter, or rollout framework remains in any diff.
