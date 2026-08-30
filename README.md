# Lenso Project Portfolio Plugin

A removable, PostgreSQL-backed initiative and project-portfolio backend for
Lenso Apps. It owns initiatives, their ordered project membership, caller-
supplied project snapshots, structured health updates, and deterministic
rollups. It does not own Projects, Teams, Issues, Organizations, identities, or
Access Control policy.

## Capabilities

The linked Plugin provides:

- `lenso.project-portfolio@1`: create/get/list/update initiatives, list attached
  project snapshots, add/list structured health updates, and read rollups.
- `lenso.project-portfolio-admin@1`: archive initiatives, attach/detach/reorder
  projects, and refresh an attached project snapshot.

It requires exactly one Provider for each of `lenso.secrets@1`,
`lenso.organization-membership@1`, and `lenso.access-control@1`.

Every request requires an exact configured caller, an Auth Actor Assertion
audienced to the exact Capability operation, live Organization membership, and
an independent Access Control decision. Permissions are
`portfolio.initiatives.read`, `portfolio.initiatives.write`, and
`portfolio.initiatives.admin`.

## Project snapshot boundary

Project IDs are opaque strings. The Plugin never reads a Projects database or
imports Projects implementation types. An authorized caller supplies a bounded
snapshot when attaching or refreshing a project: display name, status category,
health, progress, target window, source revision, and observation time. The
Plugin persists that evidence and computes rollups only from its owned
snapshots. A caller that needs current Projects validation must do so through
`lenso.projects@1` before invoking this Plugin; Portfolio deliberately does not
declare a dependency it cannot use transactionally.

## Consistency and lifecycle

- Mutations use caller/actor/operation-scoped idempotency keys.
- Mutable initiatives and memberships use positive decimal CAS revisions.
- Reorder accepts the complete current membership, validates every revision,
  and commits atomically.
- An initiative owns at most 500 current project snapshots, matching the bounded
  full-reorder contract.
- Structured initiative updates advance initiative health, progress, and
  revision in the same transaction.
- Rollups report their source as `owned_project_snapshots`.
- `ProjectPortfolioOperator::setup/upgrade` owns DDL. Runtime activation only
  resolves the database URL and verifies the exact migration ledger.
- PostgreSQL is the sole durable state; there is no memory fallback.

## Verification

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
./scripts/check-repository-boundary.sh
LENSO_PACKAGE_ALLOW_DIRTY=1 ./scripts/check-public-packages.sh
```

Set `LENSO_PROJECT_PORTFOLIO_TEST_DATABASE_URL` to a dedicated PostgreSQL
database whose name starts with `lenso_project_portfolio_test` to run the real
restart/idempotency/CAS/snapshot/rollup acceptance slice.

## Honest v1 limits

There is no UI Contribution, arbitrary workflow engine, live Projects join,
notification delivery, or automatic stale-snapshot refresh. Rollups are
deliberately evidence-based and can be stale until an authorized caller submits
a newer snapshot.
