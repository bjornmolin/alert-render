// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! The composition root: the only place allowed to know which adapter
//! satisfies which port.

fn main() {
    // Nothing to wire yet. The listener, the template engine and the room
    // client all arrive with their beads; see `bd ready`.
    eprintln!("alert-render: no adapters wired yet");
}
