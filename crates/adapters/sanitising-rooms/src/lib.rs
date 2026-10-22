// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Sanitising every Message on its way to a Room.
//!
//! A DECORATOR ON THE `Rooms` PORT, which is what `ar-1kx.3` decided and why.
//! Three things fall out of it, and none needs code anywhere else:
//!
//! - EVERY Message passes through, fallbacks included, because the Courier
//!   delivers through this one port. Sanitising the values going INTO a
//!   template would miss two things: a template is untrusted too, so an
//!   override written into a template literal passes straight through -- and
//!   the fallback never goes through a template at all, which would leave the
//!   one path that runs when everything else failed as the one path
//!   unsanitised.
//! - the platform-wide toggle becomes "is this wired", a composition-root
//!   decision rather than a flag threaded through a use case.
//! - neither the domain nor the Courier learns the toggle exists, which is the
//!   test of whether the placement is right.

use application::delivering::{DeliveryFailure, Rooms};
use async_trait::async_trait;
use domain::rendering::Message;
use domain::sanitising::sanitised;

/// Wraps a `Rooms`, sanitising both bodies of every Message first.
pub struct Sanitising<T> {
    inner: T,
}

impl<T> Sanitising<T> {
    /// Wrap `inner`.
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<T: Rooms> Rooms for Sanitising<T> {
    async fn deliver(&self, room_key: &str, message: &Message) -> Result<(), DeliveryFailure> {
        let html = sanitised(message.html());
        let text = sanitised(message.text());

        // IF SANITISING EMPTIES A BODY, SEND THE ORIGINAL. A Message whose
        // every character was a control would otherwise become blank, and
        // Message::new refuses blank -- so there would be nothing to deliver
        // and the room would go quiet. Delivering the unsanitised original is
        // the lesser harm: it is visible, and a room that receives something
        // odd is better than one that receives nothing.
        let cleaned = Message::new(&html, &text).unwrap_or_else(|_| message.clone());

        self.inner.deliver(room_key, &cleaned).await
    }
}

#[cfg(test)]
mod tests {
    use super::Sanitising;
    use application::delivering::{DeliveryFailure, Rooms};
    use async_trait::async_trait;
    use domain::rendering::Message;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recording(Mutex<Vec<Message>>);

    #[async_trait]
    impl Rooms for Recording {
        async fn deliver(&self, _key: &str, message: &Message) -> Result<(), DeliveryFailure> {
            self.0.lock().expect("not poisoned").push(message.clone());
            Ok(())
        }
    }

    async fn delivered(html: &str, text: &str) -> Message {
        let rooms = Sanitising::new(Recording::default());
        let message = Message::new(html, text).expect("well-formed");
        rooms.deliver("k", &message).await.expect("delivered");
        rooms.inner.0.lock().expect("not poisoned")[0].clone()
    }

    /// The reason this decorator exists: a bidirectional override reaching a
    /// room makes the displayed text disagree with its bytes, and a human
    /// decides whether to act on what is displayed.
    #[tokio::test]
    async fn a_bidi_override_is_removed_from_both_bodies() {
        let got = delivered("disk \u{202e}lluf si", "disk \u{202e}lluf si").await;
        assert_eq!(got.html(), "disk lluf si");
        assert_eq!(got.text(), "disk lluf si");
    }

    /// Swedish text is ordinary text here, and a decorator that broke it would
    /// make most real alerts unreadable.
    #[tokio::test]
    async fn swedish_text_passes_through_untouched() {
        let got = delivered("Åke på Öland", "Åke på Öland").await;
        assert_eq!(got.html(), "Åke på Öland");
        assert_eq!(got.text(), "Åke på Öland");
    }

    /// THE CASE THAT WOULD SILENCE A ROOM. If every character is strippable the
    /// sanitised body is blank, `Message::new` refuses blank, and a decorator
    /// that propagated that error would deliver nothing at all.
    #[tokio::test]
    async fn a_message_that_sanitises_to_nothing_is_still_delivered() {
        let got = delivered("\u{202e}\u{200b}", "\u{202e}\u{200b}").await;
        assert_eq!(
            got.html(),
            "\u{202e}\u{200b}",
            "the original should have been sent rather than nothing"
        );
    }
}
