#!/usr/bin/env bash
set -euo pipefail

expected_crates=$'lenso-capability-project-portfolio\nlenso-capability-project-portfolio-admin\nlenso-project-portfolio-postgres-plugin'
actual_crates="$(find crates -mindepth 2 -maxdepth 2 -name Cargo.toml -print0 | xargs -0 sed -n 's/^name = "\([^"]*\)"/\1/p' | LC_ALL=C sort)"
[[ "$actual_crates" == "$expected_crates" ]] || { printf 'unexpected workspace crate boundary\n%s\n' "$actual_crates" >&2; exit 1; }

if rg -n 'path\s*=\s*"(\.\./\.\./|/)' --glob Cargo.toml .; then
  echo 'cross-repository or absolute path dependency found' >&2
  exit 1
fi
if rg -n 'lenso-capability-projects|CREATE TABLE (projects|issues|teams)' crates --glob '!**/generated.rs'; then
  echo 'Portfolio crossed the opaque Projects snapshot boundary' >&2
  exit 1
fi
if rg -n 'HashMap|Mutex<.*Vec|memory fallback|in.memory' crates --glob '*.rs'; then
  echo 'ambient in-memory durable state found' >&2
  exit 1
fi
if rg -n 'lenso-platform-|lenso-module-|HostBuilder|HostLinkedModule|ModuleManifest' Cargo.toml crates README.md docs --glob '!**/generated.rs'; then
  echo 'legacy Lenso API found' >&2
  exit 1
fi
for capability in lenso.project-portfolio@1 lenso.project-portfolio-admin@1 lenso.secrets@1 lenso.organization-membership@1 lenso.access-control@1; do
  rg -q "$capability" README.md docs crates || { echo "missing documented Capability: $capability" >&2; exit 1; }
done
