// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! The inbound half: one route per Binding, plus `/health`.

pub mod payload;

use std::sync::Arc;

use application::delivering::{Courier, DeliveryFailure, Renderer, Rooms};
use axum::Router;
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{get, post};
use domain::binding::Bindings;

use payload::arrived;

/// The largest body this accepts.
///
/// Checked BEFORE parsing, by the layer rather than by this code: a limit
/// applied to a parsed value has already allocated whatever arrived.
/// Alertmanager's `maxAlerts` bounds a group on the sender's side, and this is
/// the backstop for when nobody set it.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Everything a request needs.
struct Serving<R: Renderer, M: Rooms> {
    courier: Courier<R, M>,
    bindings: Bindings,
}

/// The router: one route per Binding, plus `/health`.
///
/// THE STATUS CODE IS THE RETRY CONTRACT, which is the whole risk in this
/// adapter. Alertmanager retries a `5xx` off its persisted notification log and
/// treats a `4xx` as permanent, so the mapping is not a matter of taste:
///
/// - a Room that refused, was unreachable, timed out, or could not be
///   addressed is `5xx`. Try again.
/// - a body that cannot be parsed, or a path with no Binding, is `4xx`.
///   Retrying an unparseable body forever helps nobody.
///
/// AND A FALLBACK IS STILL A SUCCESS. The template failed, the Room received
/// the raw notification, the delivery happened -- so `2xx`. Answering `5xx`
/// because rendering went wrong would make Alertmanager re-send something that
/// will fall back again, forever, filling the room with duplicates of one
/// fallback.
pub fn router<R, M>(courier: Courier<R, M>, bindings: Bindings) -> Router
where
    R: Renderer + 'static,
    M: Rooms + 'static,
{
    let serving = Arc::new(Serving { courier, bindings });

    let mut router = Router::new().route("/health", get(health));

    // A route per Binding, so a path with no Binding is axum's own 404 and
    // never reaches a handler that has to guess.
    for binding in serving.bindings.iter() {
        let listener = binding.listener().to_owned();
        let shared = Arc::clone(&serving);
        let key = listener.clone();
        router = router.route(
            &listener,
            post(move |body: Bytes| {
                let shared = Arc::clone(&shared);
                let key = key.clone();
                async move { notify(shared, &key, body).await }
            }),
        );
    }

    router.layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Liveness and readiness.
///
/// FROM THE PROCESS ALONE, deliberately. A probe that depended on the Room
/// being reachable would take a healthy renderer out of service every time a
/// chat server hiccupped -- and restarting would not help, because the thing
/// that is down is somewhere else. This service holds no database and no
/// queue, so there is nothing else for readiness to reflect.
async fn health() -> &'static str {
    "ok"
}

async fn notify<R, M>(serving: Arc<Serving<R, M>>, listener: &str, body: Bytes) -> StatusCode
where
    R: Renderer,
    M: Rooms,
{
    let Some(binding) = serving.bindings.for_listener(listener) else {
        return StatusCode::NOT_FOUND;
    };

    let Ok(arrived) = arrived(&body) else {
        // 4xx: retrying a body we cannot read will never succeed, and
        // Alertmanager treats this as permanent, which is what we want.
        return StatusCode::BAD_REQUEST;
    };

    match serving
        .courier
        .carry(binding, &arrived.notification, &arrived.as_sent)
        .await
    {
        // A fallback is in here too, and it is a success: the room heard
        // something and said yes.
        Ok(_) => StatusCode::OK,
        // 5xx so Alertmanager retries off its notification log. Never 4xx:
        // that would tell it to stop, and the room would stay quiet.
        Err(DeliveryFailure::Unaddressable) => StatusCode::INTERNAL_SERVER_ERROR,
        Err(_) => StatusCode::BAD_GATEWAY,
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_BODY_BYTES, router};
    use application::delivering::{Courier, DeliveryFailure, Renderer, Rooms};
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use domain::binding::{Binding, Bindings};
    use domain::notification::Notification;
    use domain::rendering::{FallbackReason, Message};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    struct Fixed(Result<Message, FallbackReason>);

    #[async_trait]
    impl Renderer for Fixed {
        async fn render(&self, _id: &str, _n: &Notification) -> Result<Message, FallbackReason> {
            self.0.clone()
        }
    }

    struct Counting {
        answer: Result<(), DeliveryFailure>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Rooms for Counting {
        async fn deliver(&self, _key: &str, _m: &Message) -> Result<(), DeliveryFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer
        }
    }

    const BODY: &str =
        r#"{"alerts":[{"status":"firing","labels":{"alertname":"DiskFull"},"annotations":{}}]}"#;

    fn bindings() -> Bindings {
        Bindings::new(vec![
            Binding::new("/notify/team", "team", "team-room-url").expect("well-formed"),
        ])
        .expect("one binding is a set")
    }

    fn serving(
        render: Result<Message, FallbackReason>,
        deliver: Result<(), DeliveryFailure>,
    ) -> (axum::Router, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let rooms = Counting {
            answer: deliver,
            calls: Arc::clone(&calls),
        };
        (
            router(Courier::new(Fixed(render), rooms), bindings()),
            calls,
        )
    }

    fn a_message() -> Message {
        Message::new("<b>x</b>", "x").expect("well-formed")
    }

    async fn post(app: axum::Router, path: &str, body: &str) -> StatusCode {
        app.oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .expect("a request"),
        )
        .await
        .expect("a response")
        .status()
    }

    #[tokio::test]
    async fn a_delivered_notification_answers_two_hundred() {
        let (app, calls) = serving(Ok(a_message()), Ok(()));
        assert_eq!(post(app, "/notify/team", BODY).await, StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// A path with no Binding is the server's own 404, and nothing is
    /// delivered -- asserted on the counter, because "404" alone would also be
    /// true of a handler that delivered first and then answered badly.
    #[tokio::test]
    async fn an_unconfigured_path_is_not_found_and_delivers_nothing() {
        let (app, calls) = serving(Ok(a_message()), Ok(()));
        assert_eq!(
            post(app, "/notify/teamx", BODY).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// THE RETRY CONTRACT, in the direction that costs most if it is wrong.
    /// Alertmanager retries a 5xx off its persisted notification log and treats
    /// a 4xx as permanent, so every room failure must be 5xx -- and the test
    /// asserts NOT 4xx explicitly, because a 4xx here means the room stays
    /// quiet and nothing ever tries again.
    #[tokio::test]
    async fn every_room_failure_is_a_server_error_so_alertmanager_retries() {
        for failure in [
            DeliveryFailure::Refused,
            DeliveryFailure::Intercepted,
            DeliveryFailure::Unreachable,
            DeliveryFailure::TimedOut,
            DeliveryFailure::Unaddressable,
        ] {
            let (app, _) = serving(Ok(a_message()), Err(failure));
            let status = post(app, "/notify/team", BODY).await;

            assert!(
                status.is_server_error(),
                "{failure:?} answered {status}, which Alertmanager would not retry"
            );
            assert!(
                !status.is_client_error(),
                "{failure:?} answered {status}, which tells Alertmanager to stop"
            );
        }
    }

    /// A FALLBACK IS STILL A SUCCESS. Answering 5xx because rendering failed
    /// would make Alertmanager re-send something that falls back again,
    /// forever, filling the room with duplicates of one fallback.
    #[tokio::test]
    async fn a_fallback_that_was_delivered_still_answers_two_hundred() {
        let (app, calls) = serving(Err(FallbackReason::TemplateMalformed), Ok(()));
        assert_eq!(post(app, "/notify/team", BODY).await, StatusCode::OK);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the room heard the fallback"
        );
    }

    /// A body we cannot read is 4xx, because retrying it will never help --
    /// the opposite direction from a room failure, and the pair is the point.
    #[tokio::test]
    async fn a_body_that_is_not_a_payload_is_a_client_error() {
        for bad in ["not json", "[1,2,3]", "{}", r#"{"alerts":[]}"#] {
            let (app, calls) = serving(Ok(a_message()), Ok(()));
            let status = post(app, "/notify/team", bad).await;
            assert!(
                status.is_client_error(),
                "body {bad:?} answered {status}, which Alertmanager would retry for ever"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }

    /// Rejected by the limit layer, so the body is never parsed and never
    /// fully allocated. A limit applied after parsing has already paid for
    /// whatever arrived.
    #[tokio::test]
    async fn a_body_over_the_limit_is_rejected_without_being_parsed() {
        let (app, calls) = serving(Ok(a_message()), Ok(()));
        let huge = format!(
            r#"{{"alerts":[{{"status":"firing","labels":{{"pad":"{}"}},"annotations":{{}}}}]}}"#,
            "x".repeat(MAX_BODY_BYTES + 1024)
        );

        let status = post(app, "/notify/team", &huge).await;

        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "got {status}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// Health answers from the process alone. A probe that depended on the
    /// Room would take a healthy renderer out of service every time a chat
    /// server hiccupped -- and restarting would not fix something that is down
    /// somewhere else.
    #[tokio::test]
    async fn health_answers_ok_while_every_room_is_unreachable() {
        let (app, _) = serving(Ok(a_message()), Err(DeliveryFailure::Unreachable));

        let response = app
            .oneshot(
                Request::get("/health")
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");

        assert_eq!(response.status(), StatusCode::OK);
    }
}
