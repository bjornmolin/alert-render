<!--
SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later
-->

# Contributing

## The loop

```sh
just quick     # architecture + tests, fast enough for every save
just verify    # the whole gate, the same thing CI runs
```

`just --list` shows the rest. There is deliberately no separate CI recipe: the
pull-request workflow runs `just check`, so there is one definition of green
rather than a copy that drifts from it.

## Two rules that are enforced rather than trusted

**Dependencies point inward.** `crates/architecture/tests/boundaries.rs` fails
the build if they do not. `domain` has zero dependencies and keeps them;
`application` gets `thiserror` and `async-trait` and nothing else. Widening a
boundary means editing `ALLOWED` in that test, so it shows up in review as a
decision rather than as an accident in a `Cargo.toml`.

**Production code arrives only as the answer to a failing test.** Not because
tests are virtuous, but because a test written afterwards tests the code you
wrote while a test written first tests the behaviour you wanted. The loop is in
`.claude/skills/red-green-refactor/`; the rest of the conventions are in
`CLAUDE.md` and the vocabulary is in `GLOSSARY.md`.

## Commits

Conventional Commits (`type(scope): description`, lowercase first letter), with
`Refs: EUDIW-XXXX` when a Jira issue tracks the change. Signed and signed-off:

```sh
git commit -s -S
```

## How this repository is checked, and where it differs from the template

Digg's [open-source-project-template][tmpl] is the house standard. Two
deliberate departures, both recorded here because the obvious correction to
either one is backwards:

**The CI generation is newer.** The template pins `diggsweden/reusable-ci`
v2.8.3. This repository uses `diggsweden/forge-ci` v0.90.0, which supersedes it
and has a different interface — `project-type`, `lint-engine` and
`required-lints` rather than reusable-ci's `linters.*` booleans. Do not "align"
this backwards to match the template.

**Three of the template's config files are absent on purpose**, because
forge-ci's lint engine already runs those tools from a container with its own
pinned versions. `nanolinter.toml` declares the plan instead:

| Template file | Replaced by |
| --- | --- |
| `.gommitlint.yaml` | the `commits` check |
| `.rumdl.toml` | the `markdown` check |
| `.mise.toml` | not needed — the container carries the tool versions |

The cost is that those checks run in CI and not locally. `just verify` covers
the Rust side, which is the half that fails most often; the hygiene half is
cheap to see in a pull request. `nanolinter.toml` also explains why the Rust
checks are *not* delegated to that engine.

[tmpl]: https://github.com/diggsweden/open-source-project-template

## Reporting something sensitive

There is no `SECURITY.md` yet, and that is deliberate rather than an oversight:
a disclosure route nobody reads is worse than none at all. Until this repository
moves to the Digg organisation and a real route exists, raise anything sensitive
privately with the maintainer rather than in a public issue.
