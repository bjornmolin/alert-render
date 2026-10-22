// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! The Message built when no Template could produce one.

use crate::rendering::{FallbackReason, Message};

/// The largest Message this builds, in bytes of either body.
///
/// **NEEDS CONFIRMING AGAINST THE REAL INSTANCE.** The Matrix specification
/// caps a single event at 65536 bytes TOTAL, and that total includes the event
/// envelope and both of our bodies, not just one. 48 KiB per body is therefore
/// a deliberate underestimate rather than the spec figure: two bodies plus
/// envelope must fit, and a hookshot deployment may cap lower still. Check
/// against `hookshot.digg.safos.se` before raising it; lowering it is always
/// safe.
///
/// Getting this wrong in the generous direction is the bad direction: the post
/// is rejected, the fallback produces nothing, and the room goes quiet -- which
/// is what the fallback exists to prevent.
pub(crate) const MAX_MESSAGE_BYTES: usize = 48 * 1024;

/// How to say each reason to whoever is reading the room.
///
/// Written for the person who can act on it -- the tenant whose template it is
/// -- rather than as an error code. "Could not" leads, so the first words say
/// this is not the service malfunctioning.
fn why(reason: FallbackReason) -> &'static str {
    match reason {
        FallbackReason::TemplateMissing => {
            "alert-render could not render this notification: no template is mounted."
        }
        FallbackReason::TemplateMalformed => {
            "alert-render could not render this notification: the template could not be parsed."
        }
        FallbackReason::RenderFailed => {
            "alert-render could not render this notification: the template failed while rendering."
        }
    }
}

/// Escapes the four characters that can change the meaning of HTML.
///
/// `&` FIRST, and the order is the whole correctness of this function: replace
/// `<` before `&` and the `&` of `&lt;` gets escaped again, so the reader sees
/// `&amp;lt;` instead of a `<`. Done by one pass over the characters rather
/// than four `replace` calls, which cannot get the order wrong at all.
fn escaped(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// Both bodies for a given prefix of the Alerts.
///
/// The marker is OUR text, not tenant data, so it needs no escaping -- and it
/// sits outside the `<pre>` so a reader sees it as a note rather than as part
/// of the payload.
fn bodies(reason: FallbackReason, shown: &[String], total: usize) -> (String, String) {
    let body = shown.join("\n");
    let (text_marker, html_marker) = if shown.len() < total {
        (
            format!("\n... truncated: showing {} of {total} alerts", shown.len()),
            format!(
                "<br>... truncated: showing {} of {total} alerts",
                shown.len()
            ),
        )
    } else {
        (String::new(), String::new())
    };

    let text = format!("{}\n{body}{text_marker}", why(reason));
    // The text body is NOT escaped: it is plain text, and escaping it would
    // show `&lt;` to a reader whose client prefers that field.
    let html = format!(
        "{}<br><pre>{}</pre>{html_marker}",
        why(reason),
        escaped(&body)
    );
    (html, text)
}

/// The Message to send when no Template could produce one.
///
/// `raw_alerts` is one serialised Alert per element. The SERIALISING is an
/// adapter's job -- this crate has no serde -- and the per-Alert chunking is
/// deliberate rather than incidental: truncating one long JSON string would cut
/// mid-object, and the marker could then not honestly say how many Alerts a
/// reader is looking at.
///
/// Infallible on purpose. The one path that exists for when everything else
/// failed must not have a failure mode of its own.
///
/// # Panics
///
/// Cannot. `Message::new` only refuses a blank body, and both bodies always
/// begin with the reason line, which is a non-empty literal. The `expect` is
/// there because the constructor is the only way to build a `Message` and its
/// invariant is genuinely satisfied here, not because the case is unhandled.
#[must_use]
pub fn fallback(reason: FallbackReason, raw_alerts: &[String]) -> Message {
    let total = raw_alerts.len();

    // WHOLE ALERTS, LARGEST PREFIX THAT FITS. Counting down one Alert at a
    // time rather than binary-searching: a Notification carries tens of Alerts,
    // not thousands, so the simple version is readable and the clever version
    // would be the same speed. Zero always fits -- the reason line and the
    // marker are a few hundred bytes -- so this terminates.
    for shown in (0..=total).rev() {
        let (html, text) = bodies(reason, &raw_alerts[..shown], total);
        if html.len() <= MAX_MESSAGE_BYTES && text.len() <= MAX_MESSAGE_BYTES {
            return Message::new(&html, &text).expect("the reason line is never blank");
        }
    }

    unreachable!("the empty prefix is a few hundred bytes and cannot exceed the cap")
}

#[cfg(test)]
mod tests {
    use super::fallback;
    use crate::rendering::FallbackReason;

    /// THE OTHER HALF, and the one that catches a marker bolted on
    /// unconditionally: a Notification that fits must carry no marker at all.
    /// A permanent "truncated" note would teach every reader to ignore it, so
    /// the day it is true nobody would look.
    #[test]
    fn a_fallback_that_fits_carries_no_truncation_marker() {
        let small = vec![
            r#"{"status":"firing","alertname":"DiskFull"}"#.to_owned(),
            r#"{"status":"resolved","alertname":"CpuHot"}"#.to_owned(),
        ];
        let message = fallback(FallbackReason::TemplateMalformed, &small);

        for body in [message.html(), message.text()] {
            assert!(!body.contains("truncated"), "{body}");
            assert!(!body.contains(" of 2"), "{body}");
        }
        // Both alerts are actually there, not just unmarked.
        assert!(message.text().contains("DiskFull"));
        assert!(message.text().contains("CpuHot"));
    }

    /// A single Alert is the common case and the one most likely to be built by
    /// a special path by accident. The wrapper must still be balanced and the
    /// content still escaped.
    #[test]
    fn a_single_alert_fallback_is_still_well_formed_html() {
        let message = fallback(
            FallbackReason::RenderFailed,
            &[r#"{"summary":"5 > 3 & \"quoted\""}"#.to_owned()],
        );

        let html = message.html();
        assert_eq!(html.matches("<pre>").count(), 1, "{html}");
        assert_eq!(html.matches("</pre>").count(), 1, "{html}");
        assert!(
            html.find("<pre>") < html.find("</pre>"),
            "closed before opened: {html}"
        );
        // Every raw angle bracket outside our own tags is gone.
        let inside = html
            .split_once("<pre>")
            .and_then(|(_, rest)| rest.split_once("</pre>"))
            .map(|(inside, _)| inside)
            .expect("the wrapper is there");
        assert!(
            !inside.contains('<'),
            "unescaped < inside the wrapper: {inside}"
        );
        assert!(
            !inside.contains('>'),
            "unescaped > inside the wrapper: {inside}"
        );
        assert!(inside.contains("&gt;") && inside.contains("&amp;") && inside.contains("&quot;"));
    }

    /// WHY TRUNCATION IS NOT OPTIONAL: without it the failure path has its own
    /// failure path. Matrix caps an event at roughly 64 KiB, so an untruncated
    /// fallback for a large group would simply fail to post and the room would
    /// go quiet -- the exact outcome this whole mechanism exists to prevent.
    ///
    /// The marker must be HONEST about the count, which is why whole Alerts are
    /// included rather than bytes: "showing 2 of 40" is checkable, whereas a
    /// byte-truncated blob cannot say how many Alerts a reader is looking at.
    #[test]
    fn an_oversized_fallback_is_truncated_and_says_how_many_it_shows() {
        let fat: Vec<String> = (0..40)
            .map(|i| format!(r#"{{"n":{i},"pad":"{}"}}"#, "x".repeat(4096)))
            .collect();
        let message = fallback(FallbackReason::TemplateMissing, &fat);

        assert!(
            message.html().len() <= super::MAX_MESSAGE_BYTES,
            "html is {} bytes, over the cap",
            message.html().len()
        );
        for body in [message.html(), message.text()] {
            assert!(
                body.contains(" of 40"),
                "no honest count in: {}",
                &body[..200.min(body.len())]
            );
            assert!(body.contains("truncated"), "no marker");
        }
        // And it still says why it fell back, which must survive truncation.
        assert!(message.text().contains("no template is mounted"));
    }

    /// THE FALLBACK DOES ITS OWN ESCAPING, and this is the test that matters
    /// most in the file. It cannot lean on the template engine's auto-escaping,
    /// because the engine may be exactly what failed -- so the one path that
    /// runs when rendering broke is also the one path with hand-rolled
    /// escaping, which is a combination worth a test rather than a comment.
    ///
    /// An annotation is tenant data and can contain anything. Unescaped, a
    /// `</pre>` in it closes the wrapper and everything after is markup.
    #[test]
    fn alert_content_cannot_break_out_of_the_wrapper() {
        let nasty = r#"{"summary":"</pre></div><script>alert(1)</script>","q":"\"x\" & y"}"#;
        let message = fallback(FallbackReason::RenderFailed, &[nasty.to_owned()]);

        let html = message.html();
        // Exactly one <pre> opened and one closed: the wrapper, and nothing else.
        assert_eq!(html.matches("<pre>").count(), 1, "{html}");
        assert_eq!(html.matches("</pre>").count(), 1, "{html}");
        assert!(!html.contains("<script>"), "{html}");
        assert!(!html.contains("</div>"), "{html}");
        assert!(html.contains("&lt;/pre&gt;"), "{html}");
        assert!(html.contains("&amp;"), "{html}");

        // The text body is plain text and is NOT html-escaped -- escaping it
        // would show `&lt;` to a reader of a client that prefers text.
        assert!(
            message.text().contains("</pre></div>"),
            "{}",
            message.text()
        );
    }

    /// THE REASON MUST REACH A HUMAN, and in both bodies. A client that prefers
    /// `text` would otherwise show raw JSON with no explanation, which reads as
    /// the service malfunctioning rather than as this tenant's template needing
    /// a fix -- and the tenant is the only one who can fix it.
    #[test]
    fn the_fallback_says_why_it_fell_back_in_both_bodies() {
        let message = fallback(
            FallbackReason::TemplateMalformed,
            &[r#"{"status":"firing"}"#.to_owned()],
        );

        for body in [message.html(), message.text()] {
            assert!(
                body.contains("template"),
                "the reason is not in this body: {body}"
            );
        }
        assert!(message.text().contains("could not"), "{}", message.text());
    }
}
