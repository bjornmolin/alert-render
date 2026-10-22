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

# ── The container ───────────────────────────────────────────────────────────
#
# A release builds this through forge-ci (see .github/workflows/release.yml);
# these are for building and checking it by hand.

image tag="alert-render:local":
    docker build -f Containerfile --target runtime -t {{tag}} .

# The properties the manifests rely on, checked by trying rather than reading.
#
# Go template syntax is avoided here on purpose: `{` doubled is `just`'s own
# interpolation, and escaping a Docker --format string through it produced
# stray braces and a check that failed on its own output rather than on the
# image. Reading the JSON is shorter than getting the escaping right.
image-verify tag="alert-render:local":
    #!/usr/bin/env bash
    set -euo pipefail
    user=$(docker image inspect {{tag}} | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["Config"]["User"])')
    echo "user: $user"
    [ "$user" = "65532:65532" ] || { echo "not running as the nonroot uid"; exit 1; }
    for shell in /bin/sh /bin/bash /bin/busybox; do
        if docker run --rm --entrypoint "$shell" {{tag}} -c true 2>/dev/null; then
            echo "$shell exists in the image"; exit 1
        fi
    done
    echo "no shell: ok"
    # It must refuse to start without a config, naming the file. The output is
    # captured BEFORE grepping: the container exits non-zero on purpose, and
    # under `pipefail` that fails the pipeline whatever grep found -- which made
    # this check report a failure of its own making.
    refusal=$(docker run --rm {{tag}} 2>&1 || true)
    echo "$refusal" | grep -q "no bindings file at" \
        || { echo "it did not refuse a missing config by name, it said: $refusal"; exit 1; }
    echo "refuses a missing config by name: ok"

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
