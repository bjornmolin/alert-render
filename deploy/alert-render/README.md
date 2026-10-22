<!--
SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later
-->

# Turning alerting on, as a tenant

This directory is a **reference set**, not a deployment. Copy it into your own
gitops repository under your namespace, change the five things below, and ArgoCD
does the rest. It is deployed *by you*, on activation — there is no platform
ticket, which is the point.

> **It will not run yet.** There is no container image: no `Containerfile` and
> no release workflow exist (bead `ar-1kx.11`). `deployment.yaml` carries a
> `REPLACE-ME@sha256:REPLACE-ME` marker rather than pretending otherwise. Pin
> by digest when there is one, not by tag.

## What you must change

| Where | What |
| --- | --- |
| `kustomization.yaml` | `namespace:` — yours |
| `deployment.yaml` | the `image:` marker, once there is an image |
| `bindings.example.yaml` | your listener paths, templates and room **keys** |
| `templates/` | how your alerts should read |
| `alertmanagerconfig.yaml` | the receiver URLs, and your routing |

And one thing you must create out of band, because it is a credential:

```sh
kubectl -n <your-namespace> create secret generic alert-render-rooms \
  --from-literal=team-room-url='https://hookshot.digg.safos.se/<room-uuid>'
```

One key per binding. **A room's URL is its entire credential** — there is no
separate token, so whoever holds it can post to that room. That is why
`bindings.example.yaml` names a *key* and never the address: that file is a
ConfigMap in a git repository, and an address in it would be published into your
git history and into `kubectl get cm -o yaml` for anyone with read on the
namespace. The service refuses an `http(s)` value in that field for exactly this
reason, so the mistake fails at startup rather than at the first alert.

## The one thing written twice

Each `listener` path appears in **two** files: `bindings.example.yaml` and the
receiver URLs in `alertmanagerconfig.yaml`. They must be the same string, and
nothing cross-checks them:

- a receiver pointing at a path with no binding gets a `404`, which Alertmanager
  retries for ever
- a binding no receiver posts to is simply never used, which looks exactly like
  *"no alerts fired"* — the dangerous one, and `ar-1kx.1` is about making it
  visible

## How often a room hears from you

Decided entirely in `alertmanagerconfig.yaml`, not here: this service forwards
every notification it is sent and suppresses nothing. `repeatInterval: 4h` means
about two messages over a six-hour incident; `5m` means about seventy. Muting a
channel is the failure this is meant to avoid, so start longer than feels right.

`maxAlerts: 50` is a backstop on the sender's side. Without it a large group
arrives whole and the renderer has to truncate it to fit a chat event; letting
Alertmanager do it is better, because it says how many it dropped.

## What the platform owns, and you do not

- **the `ServiceMonitor`** that scrapes `:9090`. Deliberately not here: if it
  were yours, a deployment could make its own delivery failures invisible by
  not being scraped.
- **egress**, where it is needed. Production tenant namespaces have no egress
  policy at all, so nothing is required there. The dev cluster is *stricter*
  than production and does need a rule to reach an off-cluster chat server —
  which is the opposite of the usual direction, and the platform's to add.
- **`alertmanagerConfigSelector`** on the Alertmanager, without which your
  `AlertmanagerConfig` is ignored. Already `{}` in the dev cluster; on OpenShift
  user-workload monitoring it needs `enableUserAlertmanagerConfig`.

## Why your routing is safe to own

The Prometheus Operator **stamps** a `namespace=<your-namespace>` matcher onto
every route it merges from your `AlertmanagerConfig`, and auto-namespaces the
receiver names. Measured in the dev cluster on 2026-10-21: the generated
configuration carried `matchers: - namespace="wallet-lab-prod"` on a route that
never asked for one. So you cannot route another tenant's alerts and you cannot
collide with another tenant's receiver — which is what makes this yours to write
rather than a platform change.

## Templates

Two per binding: `<name>.html` and `<name>.txt`. A chat client shows one or the
other, so both carry the whole message. **The `.html` one is escaped and the
`.txt` one is not** — you say nothing about escaping, the file extension does.
Use `.html` only; `.HTML` is refused, because the escaping is selected from the
lower-case name and a file that slipped through would render unescaped.

What a template may read:

```
notification.count
notification.alerts[].state         "firing" or "resolved", to print
notification.alerts[].firing        a boolean, to branch on
notification.alerts[].labels.<name>
notification.alerts[].annotations.<name>
```

A **typo fails** rather than rendering blank — a blank reaches your room looking
like an alert with no severity, and nobody investigates that. So an optional
label needs an explicit `| default("...")`, which is you saying what absence
means. The examples here do that for `severity` and `summary`.

If a template is missing, malformed, or fails, the room still hears something:
the raw notification, escaped, with the reason stated. That is deliberate and
unmistakable — if you see it, your template needs fixing, not the service.
