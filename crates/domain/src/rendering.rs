// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! What a Room receives, and the two ways it came to exist.

/// Why a Message could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedMessage {
    /// The HTML body was empty or only whitespace.
    EmptyHtml,
    /// The plain-text body was empty or only whitespace.
    EmptyText,
}

/// The `{html, text}` pair a Room receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    html: String,
    text: String,
}

impl Message {
    /// A Message with both bodies, or why it could not be one.
    ///
    /// # Errors
    ///
    /// [`MalformedMessage::EmptyHtml`] or [`MalformedMessage::EmptyText`] if
    /// that body is empty or only whitespace. Both are refused here rather than
    /// downstream because a Room cannot report a blank message as a problem --
    /// it simply shows nothing, which looks exactly like a room that went
    /// quiet.
    pub fn new(html: &str, text: &str) -> Result<Self, MalformedMessage> {
        if html.trim().is_empty() {
            return Err(MalformedMessage::EmptyHtml);
        }
        if text.trim().is_empty() {
            return Err(MalformedMessage::EmptyText);
        }
        Ok(Self {
            html: html.to_owned(),
            text: text.to_owned(),
        })
    }

    /// The HTML body.
    #[must_use]
    pub fn html(&self) -> &str {
        &self.html
    }

    /// The plain-text body.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Why a Template did not produce the Message.
///
/// A SET OF CASES RATHER THAN A STRING. Missing, malformed and failed-while-
/// rendering are different things to whoever has to fix one -- and they are
/// also the metric labels, so keeping them as text would push the distinction
/// into formatting and lose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// No Template was mounted at all.
    TemplateMissing,
    /// A Template was mounted but could not be parsed.
    TemplateMalformed,
    /// The Template parsed and then failed while rendering.
    RenderFailed,
}

/// The outcome of applying a Template to a Notification.
///
/// NOT A `Result`, AND THAT IS THE POINT. A broken Template still produces a
/// Message, so there is no error case here for a caller to mishandle. If this
/// were `Result<Message, _>` then somewhere downstream would eventually treat
/// the failure as a reason not to deliver, and a room would go quiet exactly
/// when a tenant most needed to hear from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rendering {
    /// The Template produced this Message.
    Rendered(Message),
    /// The Template did not, so this is the fallback.
    FellBack {
        /// Why the Template did not produce it.
        reason: FallbackReason,
        /// The fallback Message, which is still a Message.
        message: Message,
    },
}

impl Rendering {
    /// The Message, whichever way it came to exist.
    ///
    /// INFALLIBLE BY DESIGN -- `&Message`, never `Option<&Message>`. Both
    /// variants carry one, so a caller never has a case to forget, and the
    /// decision that a broken Template still reaches the room is enforced by
    /// the type rather than by everyone remembering it.
    #[must_use]
    pub fn message(&self) -> &Message {
        match self {
            Self::Rendered(message) | Self::FellBack { message, .. } => message,
        }
    }

    // NO `fallback_reason()` ACCESSOR YET, deliberately. One was written here
    // while making the test above pass, which made it production code that no
    // failing test had asked for -- so it went again. The metrics bead needs it
    // and will drive it; until then `FellBack` is destructured where the reason
    // is wanted, which is one call site.
}

#[cfg(test)]
mod tests {
    use super::{FallbackReason, MalformedMessage, Message, Rendering};

    /// THE CRITERION THIS BEAD EXISTS FOR, and it is checked by the compiler
    /// rather than by an assertion.
    ///
    /// Both lines below call `.message().text()` with no `unwrap`, no `expect`
    /// and no `match`. That only compiles because `message()` returns
    /// `&Message` and not `Option<&Message>` -- so the test passing at all is
    /// the proof that a Rendering carrying no Message cannot be obtained. If
    /// someone later makes the accessor fallible, this file stops compiling,
    /// which is the loudest failure available.
    #[test]
    fn every_rendering_yields_a_message_without_unwrapping_anything() {
        let rendered = Rendering::Rendered(Message::new("<b>a</b>", "a").expect("well-formed"));
        let fell_back = Rendering::FellBack {
            reason: FallbackReason::TemplateMissing,
            message: Message::new("<b>b</b>", "b").expect("well-formed"),
        };

        assert_eq!(rendered.message().text(), "a");
        assert_eq!(fell_back.message().text(), "b");
    }

    /// Both outcomes name what happened, and the fallback says why.
    #[test]
    fn a_rendering_is_either_rendered_or_a_named_fallback() {
        let message = Message::new("<b>x</b>", "x").expect("well-formed");

        let rendered = Rendering::Rendered(message.clone());
        assert!(matches!(rendered, Rendering::Rendered(_)));

        let fell_back = Rendering::FellBack {
            reason: FallbackReason::TemplateMalformed,
            message,
        };
        let Rendering::FellBack { reason, .. } = fell_back else {
            panic!("constructed a fallback and got something else");
        };
        assert_eq!(reason, FallbackReason::TemplateMalformed);
    }

    /// A Room shows one of these two fields to a human, and which one depends
    /// on the client. So both carry the whole message and neither may be
    /// blank -- a Message with an empty `text` renders as nothing at all in a
    /// client that prefers it, which is indistinguishable from a room that
    /// went quiet.
    ///
    /// Whitespace counts as blank for the same reason: a message of three
    /// spaces is a silent room with extra steps.
    #[test]
    fn a_message_needs_both_bodies_and_neither_may_be_blank() {
        let message = Message::new("<b>disk full</b>", "disk full")
            .expect("both bodies present is a well-formed message");
        assert_eq!(message.html(), "<b>disk full</b>");
        assert_eq!(message.text(), "disk full");

        assert_eq!(
            Message::new("", "disk full"),
            Err(MalformedMessage::EmptyHtml)
        );
        assert_eq!(
            Message::new("   ", "disk full"),
            Err(MalformedMessage::EmptyHtml)
        );
        assert_eq!(
            Message::new("<b>x</b>", ""),
            Err(MalformedMessage::EmptyText)
        );
        assert_eq!(
            Message::new("<b>x</b>", "\n\t "),
            Err(MalformedMessage::EmptyText)
        );
    }
}
