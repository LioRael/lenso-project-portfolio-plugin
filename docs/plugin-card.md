# Project Portfolio v1 Plugin card

## Outcome and deletion boundary

Product leaders can group opaque projects into initiatives, publish structured
health updates, retain target windows, and read a stable rollup under concurrent
edits. Removing the Plugin Instance, its bindings, and its owned schema removes
all portfolio behavior without deleting Projects or Organization facts.

## Owned facts

The Plugin owns initiatives, ordered project membership, caller-supplied project
snapshots, initiative updates, CAS revisions, command receipts, and snapshot-
based rollup inputs. It owns no Project lifecycle or external integration fact.

## Roles and authority

`lenso.project-portfolio@1` owns ordinary portfolio use.
`lenso.project-portfolio-admin@1` owns archival and membership administration.
Both require exact caller allowlists, exact-operation Auth assertions, active
membership, and Access Control. The target remains final authority over CAS,
archival, membership uniqueness, and full-order invariants.

Separate `lenso.project-portfolio.agent-tools` and
`lenso.project-portfolio-admin.agent-tools` adapters each provide
`lenso.agent.tool-provider@2` while requiring only their corresponding
Portfolio Capability. They own no facts or policy. Removing either adapter
removes only that Agent surface and leaves Portfolio state unchanged.

## Snapshot and rollup semantics

Attach and refresh accept one project snapshot with a source-owned revision and
observation timestamp. IDs remain opaque and historical snapshots remain
Portfolio-owned evidence. A rollup counts status and health, computes average
progress, and derives the outer target window only from current attached
snapshots. It never implies the source Project is still unchanged.

## Lifecycle and removal

Operator setup/upgrade owns migrations. Activation verifies the ledger and
opens a fresh generation-local pool; deactivation closes it. There are no
background tasks. Removal needs no Kernel branch or hidden registration.
