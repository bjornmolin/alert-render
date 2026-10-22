<!--
SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later
-->

# Changelog

Notable changes, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Nothing is released yet: the workspace version is `0.0.0` and there is no
release pipeline. Entries below describe what exists, not what has shipped.

## Unreleased

### Added

- The domain: notifications and alerts, messages and renderings, the fallback,
  bindings, rooms, and Unicode sanitising. No dependencies, enforced by a test.
- The `Courier` use case with its `Renderer` and `Rooms` ports, and a
  `RoomDirectory` port for resolving a binding's room.
- `mounted-rooms`: room addresses read from a mounted directory, re-read per
  delivery so a rotated address needs no restart.
- A pull-request gate: `diggsweden/forge-ci` for hygiene and security, plus
  `just check` and the architecture test as their own checks.

### Not yet built

Every adapter that touches the outside world: the YAML configuration, the
template engine, the HTTP room client, the inbound listener, metrics, and the
manifests a tenant applies.
