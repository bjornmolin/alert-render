<!--
SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later
-->

# alert-render

Renders Prometheus alerts into a chat room, using a template the tenant owns.

Alertmanager cannot do this itself. Its `webhook_configs` emits a fixed JSON
schema and has **no body template** — the fields are `url`, `urlSecret`,
`httpConfig`, `maxAlerts`, `sendResolved` and `timeout`, and that is all. Its
native Slack, Teams, Telegram, Discord, Webex and Rocketchat integrations all
template; there is no native Matrix integration and the webhook receiver cannot
be templated. So a room pointed straight at Alertmanager receives
`{"receiver":...,"status":"firing","alerts":[...]}` pasted in as a message.

This service is the one hop that fixes that, and nothing else.

## The shape

```
PrometheusRule         tenant gitops   what is worth alerting on
AlertmanagerConfig     tenant gitops   which alerts go where (operator stamps
                                       namespace="<tenant>" on the route)
Template ConfigMap     tenant gitops   how the message reads
Room URI Secret        tenant gitops   where it goes — the URI is the credential
alert-render           tenant namespace this service, deployed on activation
```

Alertmanager retries. Its notification log lives on a PersistentVolume and is
gossiped between replicas, so it is already a durable outbox — which is why this
service is **stateless**: no database, no queue, no claim/report hand-off, no
lease. A room that cannot be reached produces a non-2xx answer and Alertmanager
tries again.

That is the whole reason it can run several replicas with nothing to coordinate.

## What it deliberately is not

- **Not a webhook gateway.** No signature verification, no GitHub, no multiple
  origins. One sender: the Alertmanager of the cluster it runs in.
- **Not multi-tenant.** One deployment per tenant, in that tenant's namespace,
  so there is no tenant to derive from a request and therefore no tenant to
  derive wrongly.
- **Not a replacement for `motrice/webhook-proxy`.** That service verifies
  senders, holds a durable outbox and hands deliveries to a per-tenant agent
  across a trust boundary. It is the right shape for webhooks that arrive from
  the internet. This is the right shape for alerts that arrive from
  Alertmanager, and the two can coexist.

## The national REST API-profil does not apply

Decided in `ar-kt7`, recorded here so nobody re-derives it: the profile governs
APIs a public-sector organisation *provides* to consumers. This service has no
consumer — the only caller is the cluster's own Alertmanager, whose request
shape Prometheus dictates — no resource, no query, and no reach outside the
namespace. An OpenAPI document is still worth having for the one endpoint; it is
not a conformance claim.

## Running it

See `justfile`. `just verify` is the gate; `just quick` is the loop.

## Licence

EUPL-1.2. See `LICENSES/`.
