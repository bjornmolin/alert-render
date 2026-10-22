// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! The rules, in the language of `GLOSSARY.md`, with no I/O and no dependencies.
//!
//! Nothing here yet, deliberately: production code in this repository arrives
//! only as the answer to a failing test (`.claude/skills/red-green-refactor`).
//! The first types to land are listed in the beads under `ar-` — start with
//! `bd ready`.

pub mod sanitising;

pub mod binding;
pub mod fallback;
pub mod notification;
pub mod rendering;
pub mod room;
