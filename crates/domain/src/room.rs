// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Where a Message goes, and the credential that is the same thing.

/// Why a value is not a usable Room address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedRoom {
    /// Empty, or only whitespace.
    Blank,
    /// Not an `http://` or `https://` address.
    NotAnAddress,
}

/// A Room's webhook address.
///
/// THE ADDRESS IS THE ENTIRE CREDENTIAL -- there is no separate token, so
/// whoever holds it can post to that room. That is why this is a type rather
/// than a `String`: `Debug` and `Display` are written to redact it, so a room
/// address cannot reach a log line, an error message or a panic by someone
/// forgetting. The only way to the value is [`Room::exposed`], which is named
/// to be uncomfortable at a call site.
// NO `derive(Debug)`. The derive prints the field, which is the leak this type
// exists to prevent -- and it is the default, so it is what arrives by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Room(String);

impl core::fmt::Debug for Room {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Enough to be recognisable in a log, nothing of the value. Not even
        // the host: a room address is one URL, so the host plus a redaction
        // marker would narrow it to a guessable UUID.
        f.write_str("Room(<redacted>)")
    }
}

impl core::fmt::Display for Room {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("<redacted room address>")
    }
}

impl Room {
    /// A Room at this address.
    ///
    /// Surrounding whitespace is stripped. That is not politeness: the value
    /// arrives from a mounted file, every editor adds a trailing newline, and a
    /// secret-manager template with a stray byte either side of the value is a
    /// failure upstream has already paid for -- there it produced a 401 on every
    /// delivery with nothing in any log to explain it.
    ///
    /// # Errors
    ///
    /// [`MalformedRoom::Blank`] for an empty or whitespace-only value, which
    /// means the secret was never rendered into the file -- a mount or a
    /// template problem.
    ///
    /// [`MalformedRoom::NotAnAddress`] for anything that is not `http://` or
    /// `https://`. This catches the mistake the key indirection invites: the
    /// file holding the Binding's secret KEY (`team-room-url`) rather than the
    /// URI it names. Without the check that becomes a POST to a relative path,
    /// failing a long way from its cause.
    pub fn new(address: &str) -> Result<Self, MalformedRoom> {
        let address = address.trim();
        if address.is_empty() {
            return Err(MalformedRoom::Blank);
        }
        if !(address.starts_with("http://") || address.starts_with("https://")) {
            return Err(MalformedRoom::NotAnAddress);
        }
        Ok(Self(address.to_owned()))
    }

    /// The address itself, for the one adapter that has to post to it.
    #[must_use]
    pub fn exposed(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{MalformedRoom, Room};

    /// THE MISTAKE THIS CATCHES is the one the indirection invites: a Binding
    /// names a secret KEY, and the mounted file is supposed to hold the URI --
    /// so someone wiring it up puts the key in the file, or copies the Binding
    /// line, and the value that arrives is `team-room-url` rather than an
    /// address. Without this check that is a POST to a relative path, failing
    /// somewhere far away from the cause.
    #[test]
    fn a_room_address_must_actually_be_an_http_address() {
        assert_eq!(Room::new("team-room-url"), Err(MalformedRoom::NotAnAddress));
        assert_eq!(
            Room::new("hookshot.example.invalid/x"),
            Err(MalformedRoom::NotAnAddress)
        );
        assert!(Room::new("http://stub-hookshot.webhook-proxy.svc:8080/h/r").is_ok());
        assert!(Room::new("https://hookshot.example.invalid/r").is_ok());
    }

    /// Blank is distinct from malformed, because the fixes differ: a blank file
    /// means the secret was never rendered into it (a mount or a Bao template),
    /// while a non-address means the wrong value was put there.
    #[test]
    fn a_blank_address_is_distinct_from_a_malformed_one() {
        assert_eq!(Room::new(""), Err(MalformedRoom::Blank));
        assert_eq!(Room::new("   \n\t "), Err(MalformedRoom::Blank));
    }

    /// WHITESPACE IS STRIPPED, and this is the failure upstream paid an hour
    /// for: a Bao template with a stray byte before or after the value. There
    /// it produced a 401 on every signed delivery with nothing in any log to
    /// explain it. Here a trailing newline -- which every editor adds -- would
    /// put a newline inside the request line.
    #[test]
    fn surrounding_whitespace_is_stripped_from_an_address() {
        let room = Room::new("  https://hookshot.example.invalid/r\n").expect("well-formed");
        assert_eq!(room.exposed(), "https://hookshot.example.invalid/r");
    }

    /// THE PROPERTY THIS TYPE EXISTS FOR. A room address in a log aggregator is
    /// a credential in a log aggregator, and the usual way that happens is a
    /// `{:?}` in an error path nobody tested. So the redaction is asserted on
    /// the FORMATTED OUTPUT rather than trusted to a review of the Debug impl.
    #[test]
    fn a_room_never_shows_its_address_when_formatted() {
        let secret = "https://hookshot.example.invalid/0191a7c2-SECRET-UUID";
        let room = Room::new(secret).expect("a well-formed address");

        assert_eq!(
            room.exposed(),
            secret,
            "the adapter must still be able to post"
        );

        let debugged = format!("{room:?}");
        let displayed = format!("{room}");
        for rendering in [&debugged, &displayed] {
            assert!(
                !rendering.contains("SECRET-UUID"),
                "the address leaked: {rendering}"
            );
            assert!(
                !rendering.contains("hookshot.example.invalid"),
                "the host leaked: {rendering}"
            );
        }
        // And it still says enough to be recognisable in a log.
        assert!(debugged.to_lowercase().contains("room"), "{debugged}");
    }
}
