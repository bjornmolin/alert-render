# SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
# SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later
#
# Deliberately smaller than motrice/webhook-proxy's. That one drives
# devbase-check across a dozen hygiene linters; this repository adds them when
# it has the files to lint, and a recipe that lints nothing reports success
# dishonestly.

default:
    @just --list

# The gate. Everything a pull request must pass.
verify: check

# Fast enough to run on every save.
quick: arch test

check: fmt-check arch clippy test

# Dependency direction. Run it first: it is the cheapest failure to read.
arch:
    cargo test -p architecture

test:
    cargo test --workspace

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# ── Not yet real ────────────────────────────────────────────────────────────
#
# Each of these needs the thing it checks to exist first. They are listed
# rather than stubbed so that `just --list` shows the gap, because a recipe
# that passes by having nothing to do is the kind of green that hides work.
#
#   (lint-api is NOT coming: ar-kt7 decided the REST API-profil does not apply
#   here, and RAP-LP's own GUIDELINES.md says it checks profile 1.2.0 — so a
#   green run would be evidence about a version we never adopted.)
#   lint-license  reuse lint                        — needs the two licence
#                 texts under LICENSES/ (see its README)
#   lint-commits  gommitlint                        — needs a commit history.
#                 Note the PR gate's forge-ci job already checks commit
#                 conventions, so this recipe is for the local loop only
#   audit         cargo audit / cargo deny          — worth adding with the
#                 first real dependency, not before
#
# Already enforced, just not from here: .github/workflows/pr.yml runs this
# file's `check` on every pull request, plus diggsweden/forge-ci for secrets,
# SAST, licence and commit hygiene (bead ar-kiw).
