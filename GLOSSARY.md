<!--
SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later
-->

# Glossary

One agreed word per concept. A concept with no agreed word is a concept that
gets modelled twice. Each entry says what the word means and what it must not be
confused with — the second half is the part that earns its keep.

Add a term in the same PR that introduces the type, per
`.claude/skills/domain-modelling`.

## Notification

What Alertmanager POSTs to this service: a group of **Alerts** that its routing
decided belong together, plus the receiver name and group labels.

*Not* a single alert — a Notification almost always carries several, and
rendering one as if it carried one is the most likely first bug. *Not* a
**Message** either: a Notification is the input, a Message is the output.

**Never empty.** Alertmanager does not send an empty group, so the constructor
refuses one (`MalformedNotification::NoAlerts`). That is the only place it is
refused, and past it nothing re-checks.

## Alert

One condition that is firing or has resolved, carrying labels and annotations.

*Not* a **PrometheusRule** — that is the gitops object that defines when an
Alert exists. This service never sees the rule.

Its state is a two-case enum, never a string. The wire carries `status` as
`"firing"` or `"resolved"`; a third spelling is a parse failure
(`UnknownAlertState`), matched exactly rather than case-insensitively — invented
tolerance is where a typo stops being visible.

## Labels, Annotations

**Labels** identify an Alert (`alertname`, `severity`, `namespace`).
**Annotations** describe it to a human (`summary`, `description`,
`runbook_url`).

**Two distinct types, although the shape is identical.** They are two arguments
of the same shape to the same constructor, which is one transposition away from
a silent bug — labels rendered as descriptions, descriptions matched on as
identity. Separate newtypes make that a compile error.

**A value set to the empty string is absent.** Prometheus's own rule, not an
invention here: in PromQL a matcher of `{foo=""}` matches series that have no
`foo` at all, so the two are the same thing upstream and are the same thing in
`get`, which returns `Option<&str>`. Without it a template guarding on
`severity` renders `severity: ` for an alert whose severity was set to nothing,
and the reader cannot tell that from a real value.

## Template

The tenant's rendering instructions, authored in its own gitops repository and
mounted into this service from a ConfigMap.

**Untrusted input.** It is written outside this codebase and loaded at runtime,
so it is in the same category as a request body, not in the same category as
source code. *Not* an Alertmanager template: those are Go `text/template` and
apply to Alertmanager's own receivers, which do not include webhooks.

## Message

The `{html, text}` pair a **Room** receives.

*Not* a Notification (that is the input). webhook-proxy calls this a `Notice`;
the shape is the same because the receiving contract is the same, and the word
differs because the provenance does — ours came from a tenant's Template, not
from a renderer we own.

**Neither body may be blank, whitespace included.** A client shows one field or
the other depending on which it prefers, so a Message with an empty `text`
renders as nothing at all in some clients — indistinguishable from a room that
went quiet. Refused at the constructor (`MalformedMessage::EmptyHtml` /
`EmptyText`), because a Room cannot report a blank message as a problem.

## Rendering

The outcome of applying a **Template** to a **Notification**: either the Message
the Template produced, or a **Fallback**.

Never an error that stops a delivery. **Not a `Result`** — its `message()`
accessor returns `&Message`, not `Option<&Message>`, so there is no failure case
for a caller to mishandle. If it were fallible, something downstream would
eventually treat the failure as a reason not to deliver, and a room would go
quiet exactly when a tenant most needed to hear from it. The compiler enforces
the decision instead of everyone remembering it.

A `FellBack` names its **reason** as one of three cases — `TemplateMissing`,
`TemplateMalformed`, `RenderFailed` — and not as a string. They are different
things to whoever has to fix one, and they are the metric labels too, so text
would push the distinction into formatting and lose it.

## Fallback

The Message produced when a Template is missing, malformed, or fails while
rendering: the Notification as raw JSON, escaped and wrapped in HTML, truncated
if it must be, with the reason stated in the message itself.

*Not* a default template. A default template looks like success and can go
unnoticed for months; a Fallback is meant to be unmistakable to whoever reads
the room, because that person is the one who can fix the Template. *Not* silent
either — it is counted, and it says why.

**It escapes its own HTML.** It cannot lean on the template engine's
auto-escaping, because the engine may be exactly what failed — so the one path
that runs when rendering broke is also the one with hand-rolled escaping. An
unescaped `</pre>` in an annotation would close the wrapper and turn the rest
into markup.

**It truncates at whole Alerts, never at bytes.** Not a size optimisation: the
marker has to be able to say *"showing 2 of 40 alerts"* honestly, and a
byte-truncated blob cannot say how many Alerts a reader is looking at. Without
truncation the failure path would have a failure path of its own — an oversized
event is rejected, nothing is posted, and the room goes quiet, which is the
outcome the Fallback exists to prevent.

**A Fallback that fits carries no marker.** A permanent "truncated" note would
teach every reader to ignore it, so on the day it is true nobody would look.

## Binding, Bindings

A **Binding** is what a tenant declares: one **listener path**, the **Template**
that renders a Notification arriving there, and the **Room** it is delivered to.
A deployment serves a set of them, so one renderer can feed several rooms with
different wording.

The third element is a **secret key, never the Room URI**. A Room URI is the
whole credential and this configuration is a `ConfigMap` in a gitops repository
— a URI in it would be published into git history and into `kubectl get cm -o
yaml` for anyone with read on the namespace. The key is safe to commit; the
value is read from the mounted secret directory.

**Bindings** is a type, not a `Vec`, because the rule that matters is about the
collection: two Bindings on one listener path is ambiguous, and whichever won
would be an implementation detail nobody should have to know — the symptom would
be alerts arriving in the wrong room with the right formatting. Refused at
construction, so no request-time lookup can be ambiguous. An empty set is
refused too: a deployment that accepts nothing and delivers nowhere is a mistake,
not an idle state.

Two Bindings **may** share a Room. Two severities with two Templates landing
where one team is looking is legitimate; only the listener has to be unique.

Matching is **exact** — not by prefix, not ignoring a trailing slash.
`/notify/teamx` is a different Binding from `/notify/team`, and a prefix match
would let a Binding on `/` swallow everything including `/health`. A path with no
Binding is a 404, never a guess.

*Not* an Alertmanager `route` and *not* a Gateway API `HTTPRoute`, both of which
are called routes in the surrounding repositories. That collision is why this is
called a Binding.

## Room

The destination for a Message, identified by a webhook URI.

**The URI is the entire credential.** There is no separate token: whoever holds
the URI can post to that room, so it is handled as a secret throughout.

**It is a type, not a `String`, and the type is why.** `Debug` and `Display` are
written to redact it — there is no `derive(Debug)`, because the derive prints the
field and the derive is what arrives by accident. Not even the host survives
formatting: a room address is one URL, so a host plus a redaction marker would
narrow it to a guessable UUID. The only way to the value is `exposed()`, named to
be uncomfortable at a call site, and the redaction is asserted on formatted
output rather than trusted to a review of the impl.

**It must actually be an `http://` or `https://` address**, and that check earns
its keep against the mistake the key indirection invites: the mounted file
holding the Binding's secret *key* (`team-room-url`) instead of the URI it names.
Without it, that becomes a POST to a relative path failing a long way from its
cause. A blank value is a distinct error from a malformed one, because the fixes
differ — blank means the secret was never rendered into the file, malformed means
the wrong value was put there.

It reaches this service as a **file**, in a directory given by configuration —
never as an environment variable. An environment variable is readable from
`/proc/<pid>/environ`, is inherited by every child process, appears in
`kubectl exec -- env`, and is the thing most likely to end up in a crash dump or
an APM trace. webhook-proxy made the same choice for the same reason
(`WEBHOOK_PROXY_SECRETS_DIR`, mounted on a memory-backed `emptyDir` "so a
rendered secret never reaches a disk").

**Who writes the file is not this service's concern.** A plain Kubernetes Secret
and an OpenBao agent rendering into a tmpfs look identical through the port, so
dev can use the simpler one without the application differing from prod. That
matters here specifically: dev-mode OpenBao is in-memory and loses its contents
on restart, and webhook-proxy's committed vault-agent configuration has never
actually authenticated — its auth role is the platform's to create. Binding the
application to one provider would inherit an unproven dependency.

**It is never given to Alertmanager.** This is the point of the whole shape. An
`AlertmanagerConfig` `urlSecret` would be read by the Prometheus Operator and
inlined verbatim into the generated Alertmanager configuration in the platform's
`monitoring` namespace — measured 2026-10-21 with a canary value, which appeared
in that Secret in clear. Because this service holds the URI instead,
Alertmanager only ever learns the address of a Service in the tenant's own
namespace, and the room credential never leaves it.

## Delivery

One attempt to hand a Message to a Room, and its outcome.

*Not* retried here. Alertmanager owns the retry — its notification log is on a
persistent volume — so a Delivery that fails answers the caller honestly and
stops. This service holds no queue, no outbox and no state between requests.

## Courier

The use case: it renders a **Notification** for a **Binding** and hands the
**Message** to that Binding's **Room**, then says what happened.

**It never retries.** Alertmanager owns the retry — its notification log lives
on a persistent volume and is gossiped between replicas — so a failure is
reported to the caller, which answers the sender with a non-2xx and lets
Alertmanager try again. A retry loop here would duplicate a responsibility
already held durably elsewhere, and would hold the inbound request open while
doing it worse. In the code the whole retry policy is one `?`.

It is handed the Alerts **as sent** alongside the parsed Notification, and that
duplication is deliberate: the **Fallback** exists to show a human what could
not be rendered, so it must show what *arrived*. Re-serialising from our own
types would silently omit every field this service does not model — exactly the
field a reader might need — and it would also put serde in a layer that is
meant to be poor.

## Delivered

A Room accepted a Message, judged by that Room's own success predicate.

**Not simply 2xx.** A hookshot answers `{"ok":true}`; an inspector or a web
application firewall may answer `200` with a body of its own, which looks
exactly like success and is not. The predicate is per-destination for that
reason (webhook-proxy's bead `gc-wlx` is where this was first measured).

## Sanitising

Removing the Unicode codepoints that can make rendered text misrepresent itself.
Exactly three categories, decided in `ar-jfl`:

- the **bidirectional** formatting characters (U+061C, U+200E/F, U+202A-E,
  U+2066-9) -- they reorder what follows, so bytes and display disagree
- the **invisible** ones that are not formatting instructions: U+200B zero-width
  space and U+FEFF. `admin<ZWSP>istrator` reads as one word and compares as two
- **C0 and C1 controls**, except newline and tab, which the fallback builds its
  `text` field out of

**Not the zero-width joiners** (U+200C, U+200D). The bead asked for the joiner
and the bead was wrong: ZWJ is what holds an emoji sequence together, so
stripping it turns one glyph into three, and both joiners carry meaning in Indic
and Arabic scripts. Their abuse potential is weaker too -- a joiner cannot hide
a word boundary in Latin text, which is precisely what U+200B does.

**Not escaping non-ASCII.** `å`, `ä`, `ö`, `é` and CJK must survive untouched —
this is a Swedish public-sector service and a blunt "escape everything above
0x7F" would make most real alerts unreadable. Sanitising is also *not* HTML
escaping, which is unconditional and not a toggle.

## Tenant

The team whose alerts this instance serves. Exactly one, implied by the
namespace the instance runs in.

*Not* a field in a request. There is no tenant attribution step at all: a
deployment serves the namespace it lives in, which is why this service has no
concept of "the wrong tenant" to get wrong.
