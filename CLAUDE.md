# Mascate: agent guide

Local desktop app (Rust + GPUI) for a solo online-selling business: product discovery, own-stock resale on marketplaces, affiliate commissions. No server.

## Where things live

- `CONTEXT.md`: domain glossary. Use its terms verbatim in code, issues and PRs; read it before naming anything.
- `docs/adr/`: architecture decisions. Read the relevant ADR before touching the area it covers; record a new ADR when a structural decision is made.
- `docs/prd.md`: the plan (phases 1-3), user stories, implementation and testing decisions. Read "Implementation Decisions" and "Testing Decisions" before implementing a slice.
- `docs/commerce-integrations.md` section 5: module boundaries. `docs/credentials.html`: secrets and dev env var names.
- GitHub: PRD is issue #3, with sub-issues as slices. Issue #4 is the app skeleton; its comments (and #15) hold the GPUI dependency policy.

## Working with the owner

- The owner reads in Portuguese: write replies and PR discussion in pt-BR. Code, identifiers, commit messages and PR titles are in English.
- Commits and PR titles follow conventional commits (`feat:`, `fix:`, `docs:`, `chore:`, `test:`, `refactor:`).
- Agents drive implementation end to end; the owner decides architecture. Before a structural decision (new crate, dependency, schema shape, module boundary, cross-cutting pattern), present 2-3 options with tradeoffs and one clear recommendation, then wait.
- Reference docs for the owner are HTML pages.
- The plan is phases 1-3; refer to scope by phase.

## Issue workflow

- Slices hang off PRD #3 as sub-issues. Label `ready-for-agent` + `afk` when an agent can finish it alone, `hitl` when it needs the owner.
- Every GitHub label has its own distinct color; pick an unused one when creating a label.
- Legal and fiscal matters (CNPJ, NF-e, LGPD) are never a blocker: model them as a flag, a warning or a Reminder and keep going. Issue and PR status reflects technical state only.

## Architecture rules

- Modular monolith as a Cargo workspace, one crate per module (ADR 0002). A module reaches another only through its public interface or an internal event, and owns its own tables. An architecture test fails when a crate depends on one outside its allowed list; extend the list only with an ADR-backed decision.
- The UI crate is a thin layer over the core: screens call module interfaces and hold no business logic.
- Local libSQL-family database with versioned migrations (ADR 0003, ADR 0007). Sync is polling; no server, no webhooks.
- Every record has a UUIDv7 id, `created_at`, `updated_at` and soft delete via `deleted_at` (ADR 0004).
- Money is a decimal with an explicit currency end to end; round only for display and for values sent to a Platform.
- Stock is an append-only ledger of Stock Movements with moving-average cost (ADR 0005).
- Integrations are idempotent and reprocessable: dedupe on each Platform's natural key, so re-running a Sync or re-importing a file yields the same state.
- Restricted Features (crawlers, logged-in panel automation, anything against a Platform's terms) are built and tested but OFF behind a flag, with the reason recorded next to the flag (ADR 0006).
- Secrets live only in the OS credential store. Env vars (names in `docs/credentials.html`) are for development only. Commit only non-secret config.

## GPUI dependency policy

- Depend on `gpui-kit` from crates.io pinned to an exact version (`=x.y.z`) and commit `Cargo.lock`. GPUI itself comes in through crates.io, never vendored.
- Upgrade deliberately, in its own PR.
- Vendor only the single affected crate via `[patch.crates-io]`, and only when a trigger fires: a blocking upstream bug (for example, no-GPU/WARP rendering on Windows), a measured bottleneck, or upstream crates stop being published. First check whether a newer `gpui-pre` release already fixes it. Full rationale in the comments on issues #4 and #15.

## Platforms

Windows 11 first. Linux (Wayland, Arch/Hyprland) second, tested regularly. macOS much later; keep the core portable.

## Code style

- SOLID, YAGNI, KISS. One source of truth per rule: extract shared logic the second time it appears.
- Names carry the meaning; add a comment only for a non-obvious why (a Platform quirk, a deliberate flag, a legal reason).

## Tests

Robust tests ship with every change; they are the owner's main review signal.

- Test through a module's public interface with a real temp database, an injected clock and an injected ID generator (PRD "Testing Decisions", issue #4). Assert domain behaviour, never private functions or table shapes.
- Money, commission, fee and margin math: example tests plus property tests.
- Platform adapters: run against a fake HTTP server serving recorded fixtures, including 401/403/429, token refresh races and duplicate deliveries.
- Migrations: migrate a sample database from each released version to the current one.

## Commands

These apply once the workspace from issue #4 exists:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three are green before a PR is ready for review.

## Done means reviewed

Before calling work done, review the diff through five lenses and fix what each finds:

1. Correctness: edge cases, money rounding, idempotency, concurrency.
2. Security: secrets, input from Platforms, file paths, logs.
3. Architecture: module boundaries, ADR compliance, dependency direction.
4. Style: conventions above, clippy-clean, minimal comments.
5. Context: matches the issue, the PRD and `CONTEXT.md` vocabulary.

Summarize what each lens found in the PR description.
