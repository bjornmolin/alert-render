// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Use cases, and the ports they need stated as traits in domain language.
//!
//! A port belongs to the consumer: it says what this application *needs*, never
//! what technology will satisfy it. See `.claude/skills/port-and-adapter`.

pub mod delivering;
pub mod rooms;
