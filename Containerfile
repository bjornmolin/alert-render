# SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
# SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later
#
# A STATIC BINARY ON A BASE WITH NOTHING IN IT. Two facts make that possible and
# both were checked rather than assumed:
#
#   rustls uses `ring` here, not `aws-lc-rs`, so a musl build needs no cmake and
#   no clang (verified in Cargo.lock).
#   the binary carries its own TLS roots -- `webpki-roots` is in Cargo.lock and
#   `rustls-native-certs` is not -- so the image needs no ca-certificates layer.
#   That also means an internal CA is NOT trusted and cannot be made trusted by
#   mounting anything; see bead ar-1kx.12, which may change this file.
#
# The consequence to know before debugging one of these pods: there is no shell.
# `kubectl exec ... sh` does not work, and neither does `wget` or `curl` from
# inside. Anything that needs to scrape this pod has to come from a pod that has
# a client -- which is how wallet-iac-k3s reaches webhook-proxy, for the same
# reason.

FROM docker.io/library/rust:1.95.0-alpine3.21 AS build

# musl-dev and gcc for `ring`'s C. Pinned to the base's repository rather than
# a version, because the base image tag is the pin that matters here.
RUN apk add --no-cache musl-dev

WORKDIR /src
COPY . .

# The toolchain file names the version; the target is the only thing added.
# `uname -m` RATHER THAN $TARGETARCH, deliberately. Under buildx a foreign
# platform is emulated, so uname reports the TARGET architecture and the build
# is a native one inside emulation -- slower, but it needs no cross linker and
# no per-architecture target list. Cross-compiling musl properly would be
# faster and is the thing to reach for if release times start to hurt.
RUN cargo build --release --locked --bin alert-render \
    --target "$(uname -m)-unknown-linux-musl" \
 && cp "target/$(uname -m)-unknown-linux-musl/release/alert-render" /alert-render

# nonroot: uid 65532, declared in the image, so `runAsNonRoot: true` has
# something to find. No shell, no package manager, nothing to patch.
FROM gcr.io/distroless/static-debian12:nonroot AS runtime

COPY --from=build /alert-render /usr/local/bin/alert-render

# Both listeners. Documentation rather than enforcement -- the process reads
# ALERT_RENDER_LISTEN and ALERT_RENDER_METRICS_LISTEN -- but a reader should not
# have to find that out from the source.
EXPOSE 8080 9090

USER 65532:65532
ENTRYPOINT ["/usr/local/bin/alert-render"]
