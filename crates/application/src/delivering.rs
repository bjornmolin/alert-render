// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Getting a Notification into a Room, and saying what happened.

use async_trait::async_trait;
use domain::binding::Binding;
use domain::notification::Notification;
use domain::rendering::{FallbackReason, Message};

/// Turns a Notification into a Message using a named Template.
///
/// The port's error is [`FallbackReason`] rather than an error of its own,
/// because every way this can fail is a reason to fall back -- so the type
/// says so and the use case has nothing to translate.
#[async_trait]
pub trait Renderer: Send + Sync {
    /// Render `notification` with the Template called `template_id`.
    async fn render(
        &self,
        template_id: &str,
        notification: &Notification,
    ) -> Result<Message, FallbackReason>;
}

/// Why a Room did not receive a Message.
///
/// The four cases are measured rather than imagined; they are upstream
/// webhook-proxy's taxonomy, and `Intercepted` is the one that justifies the
/// enum: without it, something answering 2xx with a page of its own counts as a
/// delivery and a room goes quiet while every signal stays green.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFailure {
    /// The Room answered, and said no.
    Refused,
    /// Something answered successfully, but it was not the Room.
    Intercepted,
    /// Nothing answered.
    Unreachable,
    /// Something accepted the connection and then did not answer.
    TimedOut,
}

/// Hands a Message to a Room.
#[async_trait]
pub trait Rooms: Send + Sync {
    /// Deliver `message` to the Room named by `room_key`.
    ///
    /// `room_key` and not a URI: resolving the key is the adapter's business,
    /// and a port that took a URI would put a credential in the application's
    /// signatures.
    async fn deliver(&self, room_key: &str, message: &Message) -> Result<(), DeliveryFailure>;
}

/// What happened, for a caller that has to answer the sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivered {
    /// Set when the Template did not produce the Message.
    pub fell_back: Option<FallbackReason>,
}

/// Carries a Notification into a Room and reports what happened.
///
/// NEVER RETRIES. Alertmanager owns the retry -- its notification log is on a
/// persistent volume -- so a failure is reported to the caller, which answers
/// the sender with a non-2xx and lets Alertmanager try again. A retry loop here
/// would duplicate a responsibility that is already held durably elsewhere, and
/// would hold the inbound request open while doing it badly.
pub struct Courier<R: Renderer, M: Rooms> {
    renderer: R,
    rooms: M,
}

impl<R: Renderer, M: Rooms> Courier<R, M> {
    /// A Courier using these two.
    pub fn new(renderer: R, rooms: M) -> Self {
        Self { renderer, rooms }
    }

    /// Render `notification` for `binding` and hand the result to its Room.
    ///
    /// `as_sent` is the Alerts as the SENDER wrote them, one element each, and
    /// it is passed alongside the parsed `notification` on purpose rather than
    /// being derived from it. The fallback exists to show a human what could
    /// not be rendered, so it has to show what ARRIVED -- re-serialising from
    /// our own types would silently omit every field this service does not
    /// model, which is exactly the field a reader might need. It also keeps
    /// serde out of both inner layers.
    ///
    /// # Errors
    ///
    /// [`DeliveryFailure`] if the Room did not take the Message. The caller
    /// turns that into a non-2xx and Alertmanager retries; nothing is retried
    /// here.
    pub async fn carry(
        &self,
        binding: &Binding,
        notification: &Notification,
        as_sent: &[String],
    ) -> Result<Delivered, DeliveryFailure> {
        let rendered = self.renderer.render(binding.template(), notification).await;
        let (message, fell_back) = match rendered {
            Ok(message) => (message, None),
            Err(reason) => (domain::fallback::fallback(reason, as_sent), Some(reason)),
        };

        // ONE ATTEMPT. The `?` is the whole retry policy.
        self.rooms.deliver(binding.room_key(), &message).await?;
        Ok(Delivered { fell_back })
    }
}

#[cfg(test)]
mod tests {
    use super::{Courier, Delivered, DeliveryFailure, Renderer, Rooms};
    use async_trait::async_trait;
    use domain::binding::Binding;
    use domain::notification::{Alert, AlertState, Annotations, Labels, Notification};
    use domain::rendering::{FallbackReason, Message};
    use std::sync::Mutex;

    /// A Renderer that answers however the test told it to.
    struct FixedRenderer(Result<Message, FallbackReason>);

    #[async_trait]
    impl Renderer for FixedRenderer {
        async fn render(&self, _id: &str, _n: &Notification) -> Result<Message, FallbackReason> {
            self.0.clone()
        }
    }

    /// A Room that records what it was handed and how often.
    struct RecordingRoom {
        answer: Result<(), DeliveryFailure>,
        handed: Mutex<Vec<(String, Message)>>,
    }

    impl RecordingRoom {
        fn answering(answer: Result<(), DeliveryFailure>) -> Self {
            Self {
                answer,
                handed: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> usize {
            self.handed.lock().expect("not poisoned").len()
        }
        fn last(&self) -> (String, Message) {
            self.handed
                .lock()
                .expect("not poisoned")
                .last()
                .cloned()
                .expect("the room was handed something")
        }
    }

    #[async_trait]
    impl Rooms for RecordingRoom {
        async fn deliver(&self, room_key: &str, message: &Message) -> Result<(), DeliveryFailure> {
            self.handed
                .lock()
                .expect("not poisoned")
                .push((room_key.to_owned(), message.clone()));
            self.answer
        }
    }

    fn a_binding() -> Binding {
        Binding::new("/notify/team", "team", "team-room-url").expect("well-formed")
    }

    fn a_notification() -> Notification {
        Notification::new(vec![Alert::new(
            AlertState::Firing,
            Labels::from([("alertname", "DiskFull")]),
            Annotations::from([("summary", "disk is full")]),
        )])
        .expect("one alert is a notification")
    }

    /// THE DECISION THIS USE CASE EXISTS TO ENFORCE: a Template that failed
    /// does not stop a delivery. The room still hears something, it says why,
    /// and the caller is told it fell back so the fallback can be counted.
    #[tokio::test]
    async fn a_failed_template_still_reaches_the_room_as_the_fallback() {
        let room = RecordingRoom::answering(Ok(()));
        let courier = Courier::new(FixedRenderer(Err(FallbackReason::TemplateMalformed)), room);
        let as_sent = vec![r#"{"status":"firing","alertname":"DiskFull"}"#.to_owned()];

        let outcome = courier
            .carry(&a_binding(), &a_notification(), &as_sent)
            .await
            .expect("a fallback is still delivered");

        assert_eq!(
            outcome,
            Delivered {
                fell_back: Some(FallbackReason::TemplateMalformed)
            }
        );
        let (key, handed) = courier.rooms.last();
        assert_eq!(key, "team-room-url");
        assert!(
            handed.text().contains("could not render"),
            "{}",
            handed.text()
        );
        // AS SENT, not re-serialised: the raw text the sender wrote is what a
        // reader gets, including anything this service does not model.
        assert!(handed.text().contains("DiskFull"), "{}", handed.text());
    }

    /// A Room that says no is reported, and -- the part that matters -- is asked
    /// EXACTLY ONCE. Alertmanager holds the retry off a notification log on a
    /// persistent volume; a loop here would duplicate that badly and hold the
    /// inbound request open while doing it.
    #[tokio::test]
    async fn a_room_that_refuses_is_reported_and_never_asked_twice() {
        let room = RecordingRoom::answering(Err(DeliveryFailure::Refused));
        let courier = Courier::new(
            FixedRenderer(Ok(Message::new("<b>x</b>", "x").expect("well-formed"))),
            room,
        );

        let outcome = courier.carry(&a_binding(), &a_notification(), &[]).await;

        assert_eq!(outcome, Err(DeliveryFailure::Refused));
        assert_eq!(
            courier.rooms.calls(),
            1,
            "the room was asked more than once"
        );
    }

    /// INTERCEPTED IS NOT REFUSED, and conflating them is the bug upstream
    /// measured: a firewall answering 200 with a page of its own looked exactly
    /// like a delivered message, so a room went quiet while every signal
    /// upstream stayed green. The two have different fixes -- a refusal needs
    /// the room's configuration looked at, an interception needs the network
    /// path -- so they must not arrive as one value.
    #[tokio::test]
    async fn something_answering_for_the_room_is_distinct_from_a_refusal() {
        let room = RecordingRoom::answering(Err(DeliveryFailure::Intercepted));
        let courier = Courier::new(
            FixedRenderer(Ok(Message::new("<b>x</b>", "x").expect("well-formed"))),
            room,
        );

        let outcome = courier.carry(&a_binding(), &a_notification(), &[]).await;

        assert_eq!(outcome, Err(DeliveryFailure::Intercepted));
        assert_ne!(outcome, Err(DeliveryFailure::Refused));
    }

    /// The happy path, and it asserts the ROOM KEY as well as the body: a
    /// Courier that rendered correctly and delivered to the wrong room would
    /// pass a test that only looked at the message.
    #[tokio::test]
    async fn a_rendered_message_reaches_the_room_named_by_the_binding() {
        let rendered = Message::new("<b>disk is full</b>", "disk is full").expect("well-formed");
        let room = RecordingRoom::answering(Ok(()));
        let courier = Courier::new(FixedRenderer(Ok(rendered.clone())), room);

        let outcome = courier
            .carry(&a_binding(), &a_notification(), &[])
            .await
            .expect("the room accepted it");

        assert_eq!(outcome, Delivered { fell_back: None });
        let (key, handed) = courier.rooms.last();
        assert_eq!(key, "team-room-url");
        assert_eq!(handed, rendered);
        assert_eq!(courier.rooms.calls(), 1);
    }
}
