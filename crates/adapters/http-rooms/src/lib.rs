// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Handing a Message to a Room over HTTP, and judging the answer.
//!
//! DELIVERED IS NOT 2xx, and this crate is where that becomes real. A hookshot
//! answers `200` carrying `{"ok":true}` -- its own words. A web application
//! firewall, a captive portal or a webhook inspector answers `200` carrying
//! something else, and counting that as a delivery is how a room goes quiet
//! while every signal upstream stays green. Measured in a dev cluster against
//! an inspector on 2026-10-21, and filed upstream as `gc-wlx` before that.
//!
//! REDIRECTS ARE NOT FOLLOWED, which is the sharpest requirement here. The Room
//! address is the entire credential and the body is the tenant's alert text. A
//! `3xx` to another host would send both somewhere the tenant never configured
//! -- and following redirects is the DEFAULT in most HTTP clients, so this is a
//! thing to set and then to assert, not to assume.

use std::collections::BTreeMap;
use std::time::Duration;

use application::delivering::{DeliveryFailure, Rooms};
use application::rooms::RoomDirectory;
use async_trait::async_trait;
use domain::rendering::Message;

/// What this destination has to answer for a Message to count as delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceptance {
    /// `2xx` carrying `{"ok":true}` -- what a hookshot answers.
    HookshotOk,
    /// Any `2xx`, whatever the body.
    ///
    /// For a receiver that is not a hookshot: a webhook inspector cannot be
    /// made to answer `{"ok":true}` at all, because its generated sessions
    /// return an empty default body and there is no flag to change it. A
    /// GLOBAL predicate would therefore force the dev cluster to be lax, and
    /// then dev would prove a laxness production does not have -- which is why
    /// this is per-destination.
    AnySuccess,
}

/// How long a Room has to answer before the attempt is abandoned.
///
/// Set explicitly, both halves. A client with no read timeout holds the inbound
/// request open indefinitely, and Alertmanager's own webhook timeout then fires
/// first -- so the retry starts while this process is still waiting on the
/// previous attempt.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Delivers to Rooms resolved through a directory.
pub struct HttpRooms<D: RoomDirectory> {
    directory: D,
    client: reqwest::Client,
    default: Acceptance,
    per_room: BTreeMap<String, Acceptance>,
}

impl<D: RoomDirectory> HttpRooms<D> {
    /// Deliveries through `directory`, judged by `default` unless `per_room`
    /// says otherwise for that key.
    ///
    /// # Errors
    ///
    /// If the HTTP client cannot be built, which means the TLS backend did not
    /// initialise -- a startup fault, not a delivery one.
    pub fn new(
        directory: D,
        default: Acceptance,
        per_room: BTreeMap<String, Acceptance>,
    ) -> Result<Self, reqwest::Error> {
        Self::with_timeouts(directory, default, per_room, CONNECT_TIMEOUT, TOTAL_TIMEOUT)
    }

    /// As [`HttpRooms::new`], with the timeouts given rather than the constants.
    ///
    /// EXISTS FOR THE TESTS, and says so rather than pretending otherwise. The
    /// timeout test needs a server that accepts and never answers, so it waits
    /// out whatever bound it is given -- at the production value that is ten
    /// seconds in a suite meant to run on every save. One second exercises the
    /// same mechanism. The production values stay the constants above, which
    /// `new` uses and the composition root calls.
    ///
    /// # Errors
    ///
    /// If the HTTP client cannot be built.
    pub fn with_timeouts(
        directory: D,
        default: Acceptance,
        per_room: BTreeMap<String, Acceptance>,
        connect: Duration,
        total: Duration,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            // NOT FOLLOWED. See the module docs: a redirect would send the
            // credential and the tenant's text to a host nobody configured.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(connect)
            .timeout(total)
            .build()?;
        Ok(Self {
            directory,
            client,
            default,
            per_room,
        })
    }

    fn acceptance_for(&self, room_key: &str) -> Acceptance {
        self.per_room.get(room_key).copied().unwrap_or(self.default)
    }
}

#[async_trait]
impl<D: RoomDirectory> Rooms for HttpRooms<D> {
    async fn deliver(&self, room_key: &str, message: &Message) -> Result<(), DeliveryFailure> {
        // Resolved per delivery, not cached: a Room address is rotated on the
        // chat instance by somebody who is neither this service nor the tenant.
        let room = self
            .directory
            .room(room_key)
            .await
            // A room that cannot be resolved is a CONFIGURATION fault, and
            // saying `Unreachable` would send whoever is paged to look at the
            // network instead of at the mount.
            .map_err(|_| DeliveryFailure::Unaddressable)?;

        let body = serde_json::json!({
            "html": message.html(),
            "text": message.text(),
        });

        let answer = self
            .client
            .post(room.exposed())
            .json(&body)
            .send()
            .await
            .map_err(|why| {
                if why.is_timeout() {
                    DeliveryFailure::TimedOut
                } else {
                    DeliveryFailure::Unreachable
                }
            })?;

        let status = answer.status();
        if !status.is_success() {
            // A 3xx lands here too, which is correct: with redirects switched
            // off, a redirect is an answer that is not a success, and it is
            // the Room telling us to go somewhere we will not go.
            return Err(DeliveryFailure::Refused);
        }

        // 2xx IS NOT YET A DELIVERY. What the body says decides.
        match self.acceptance_for(room_key) {
            Acceptance::AnySuccess => Ok(()),
            Acceptance::HookshotOk => {
                let text = answer
                    .text()
                    .await
                    .map_err(|_| DeliveryFailure::Intercepted)?;
                let said_ok = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false);
                if said_ok {
                    Ok(())
                } else {
                    Err(DeliveryFailure::Intercepted)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Acceptance, HttpRooms};
    use application::delivering::{DeliveryFailure, Rooms};
    use application::rooms::{RoomDirectory, RoomUnavailable};
    use async_trait::async_trait;
    use domain::rendering::Message;
    use domain::room::Room;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A Room directory holding one address, handed out for any key.
    struct At(String);

    #[async_trait]
    impl RoomDirectory for At {
        async fn room(&self, _key: &str) -> Result<Room, RoomUnavailable> {
            Room::new(&self.0).map_err(|why| RoomUnavailable::Malformed {
                key: "test".to_owned(),
                why,
            })
        }
    }

    /// A receiver that answers one canned response and counts what it was sent.
    struct Stub {
        hits: Arc<AtomicUsize>,
        address: String,
    }

    /// Serves `response` verbatim to every connection. `None` accepts the
    /// connection and never answers, which is the only honest way to test a
    /// read timeout.
    async fn stub(response: Option<&'static str>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&hits);

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                let mut scratch = [0_u8; 4096];
                let _ = socket.read(&mut scratch).await;
                match response {
                    Some(answer) => {
                        let _ = socket.write_all(answer.as_bytes()).await;
                        let _ = socket.flush().await;
                    }
                    // Hold the connection open, saying nothing.
                    None => {
                        tokio::time::sleep(std::time::Duration::from_mins(2)).await;
                    }
                }
            }
        });

        Stub {
            hits,
            address: format!("http://127.0.0.1:{port}/room"),
        }
    }

    fn answering(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn a_message() -> Message {
        Message::new("<b>disk full</b>", "disk full").expect("well-formed")
    }

    fn rooms_at(address: &str, acceptance: Acceptance) -> HttpRooms<At> {
        HttpRooms::new(At(address.to_owned()), acceptance, BTreeMap::new()).expect("a client")
    }

    /// THE TEST THIS CRATE EXISTS FOR. A firewall, a captive portal or an
    /// inspector answers 200 with a page of its own, and counting that as a
    /// delivery is how a room goes quiet while every signal stays green. Three
    /// shapes, because the plausible wrong implementations differ: an HTML
    /// page, valid JSON that does not say ok, and an empty body.
    #[tokio::test]
    async fn a_two_hundred_that_is_not_the_room_is_intercepted() {
        for body in [
            "<html><body>Request blocked</body></html>",
            r#"{"status":"accepted"}"#,
            "",
        ] {
            let canned: &'static str = Box::leak(answering("200 OK", body).into_boxed_str());
            let stub = stub(Some(canned)).await;
            let rooms = rooms_at(&stub.address, Acceptance::HookshotOk);

            assert_eq!(
                rooms.deliver("team-room-url", &a_message()).await,
                Err(DeliveryFailure::Intercepted),
                "body {body:?} was accepted as a delivery"
            );
        }
    }

    /// Per-destination, and the pair is the assertion: the SAME answer is a
    /// delivery for one destination and an interception for another. A global
    /// predicate would force the dev cluster to be lax, and dev would then
    /// prove a laxness production does not have.
    #[tokio::test]
    async fn the_same_answer_is_judged_differently_per_destination() {
        let canned: &'static str =
            Box::leak(answering("200 OK", r#"{"status":"accepted"}"#).into_boxed_str());
        let stub = stub(Some(canned)).await;

        let strict = rooms_at(&stub.address, Acceptance::HookshotOk);
        assert_eq!(
            strict.deliver("team-room-url", &a_message()).await,
            Err(DeliveryFailure::Intercepted)
        );

        let lax = rooms_at(&stub.address, Acceptance::AnySuccess);
        assert_eq!(lax.deliver("team-room-url", &a_message()).await, Ok(()));

        // And an override beats the default for one key only.
        let mixed = HttpRooms::new(
            At(stub.address.clone()),
            Acceptance::HookshotOk,
            BTreeMap::from([("inspector-url".to_owned(), Acceptance::AnySuccess)]),
        )
        .expect("a client");
        assert_eq!(mixed.deliver("inspector-url", &a_message()).await, Ok(()));
        assert_eq!(
            mixed.deliver("team-room-url", &a_message()).await,
            Err(DeliveryFailure::Intercepted)
        );
    }

    /// A REDIRECT IS NOT FOLLOWED, and the assertion is that the TARGET was
    /// never contacted -- not merely that the result was an error. The room
    /// address is the whole credential and the body is the tenant's alert text;
    /// a 3xx to another host would send both somewhere nobody configured, and
    /// following redirects is the default in most clients.
    #[tokio::test]
    async fn a_redirect_is_refused_and_its_target_is_never_contacted() {
        let elsewhere = stub(Some(Box::leak(
            answering("200 OK", r#"{"ok":true}"#).into_boxed_str(),
        )))
        .await;
        let moved: &'static str = Box::leak(
            format!(
                "HTTP/1.1 302 Found
Location: {}
Content-Length: 0
Connection: close

",
                elsewhere.address
            )
            .into_boxed_str(),
        );
        let redirector = stub(Some(moved)).await;

        let rooms = rooms_at(&redirector.address, Acceptance::HookshotOk);
        let outcome = rooms.deliver("team-room-url", &a_message()).await;

        assert_eq!(outcome, Err(DeliveryFailure::Refused));
        assert_eq!(
            elsewhere.hits.load(Ordering::SeqCst),
            0,
            "the redirect target was contacted -- the credential and the alert text went with it"
        );
    }

    /// Four answers, four outcomes. 4xx is the room saying no; nothing
    /// listening is the network; accepted-and-silent is a timeout, which is the
    /// only one that needs a server that deliberately says nothing.
    #[tokio::test]
    async fn refused_unreachable_and_timed_out_are_distinct() {
        let refusing = stub(Some(Box::leak(
            answering("403 Forbidden", "no").into_boxed_str(),
        )))
        .await;
        assert_eq!(
            rooms_at(&refusing.address, Acceptance::HookshotOk)
                .deliver("k", &a_message())
                .await,
            Err(DeliveryFailure::Refused)
        );

        // A port nothing is listening on. Bound and dropped, so it is free and
        // almost certainly still unused.
        let free = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = free.local_addr().expect("addr").port();
        drop(free);
        assert_eq!(
            rooms_at(
                &format!("http://127.0.0.1:{port}/r"),
                Acceptance::HookshotOk
            )
            .deliver("k", &a_message())
            .await,
            Err(DeliveryFailure::Unreachable)
        );

        // A SHORT BOUND, not the production one. The mechanism is identical and
        // the production values are the constants `new` uses; waiting out ten
        // seconds here would make `just quick` unusable on every save.
        let silent = stub(None).await;
        let bound = std::time::Duration::from_secs(1);
        let rooms = HttpRooms::with_timeouts(
            At(silent.address.clone()),
            Acceptance::HookshotOk,
            BTreeMap::new(),
            bound,
            bound,
        )
        .expect("a client");

        let began = std::time::Instant::now();
        assert_eq!(
            rooms.deliver("k", &a_message()).await,
            Err(DeliveryFailure::TimedOut)
        );
        assert!(
            began.elapsed() < bound * 4,
            "the timeout did not bound it: {:?}",
            began.elapsed()
        );
    }

    /// A room that cannot be resolved is a configuration fault, not a network
    /// one. Saying `Unreachable` would send whoever is paged to the network.
    #[tokio::test]
    async fn a_room_that_cannot_be_resolved_is_unaddressable() {
        struct Nothing;
        #[async_trait]
        impl RoomDirectory for Nothing {
            async fn room(&self, key: &str) -> Result<Room, RoomUnavailable> {
                Err(RoomUnavailable::NoSuchKey {
                    key: key.to_owned(),
                })
            }
        }

        let rooms =
            HttpRooms::new(Nothing, Acceptance::HookshotOk, BTreeMap::new()).expect("a client");

        assert_eq!(
            rooms.deliver("team-room-url", &a_message()).await,
            Err(DeliveryFailure::Unaddressable)
        );
    }

    /// A hookshot answers `{"ok":true}`, in its own words.
    #[tokio::test]
    async fn a_room_answering_ok_true_is_a_delivery() {
        let stub = stub(Some(Box::leak(
            answering("200 OK", r#"{"ok":true}"#).into_boxed_str(),
        )))
        .await;

        let rooms = rooms_at(&stub.address, Acceptance::HookshotOk);

        assert_eq!(rooms.deliver("team-room-url", &a_message()).await, Ok(()));
        assert_eq!(stub.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod trust {
    //! What the compiled-in trust store has to contain.
    //!
    //! reqwest is built here with `rustls-tls`, which means `webpki-roots`:
    //! the PUBLIC root store, compiled into the binary. The image therefore
    //! needs no `ca-certificates` layer -- and, as the other side of the same
    //! coin, a certificate from a private CA cannot be trusted by mounting
    //! anything.
    //!
    //! MEASURED AGAINST THE REAL CHAT SERVER, 2026-10-22, from a pod in the
    //! production cluster, because walladm01 cannot reach it:
    //!
    //!   0 s:C=SE, L=HÄGERSTEN, O=Försäkringskassan, CN=*.digg.safos.se
    //!     i:C=SE, O=Telia Company AB, CN=Telia RSA OV CA v4
    //!   1 i:C=SE, O=Telia Company AB, CN=Telia RSA TLS Root CA v3
    //!   2 i:C=FI, O=Telia Finland Oyj, CN=Telia Root CA v2
    //!   3 s:C=FI, O=Telia Finland Oyj, CN=Telia Root CA v2   (self-signed)
    //!   Verify return code: 0 (ok)
    //!
    //! So it is a public commercial CA and nothing special is needed. The risk
    //! that remains is not ours to control: a root can be REMOVED from
    //! webpki-roots, and the symptom would be every delivery failing as
    //! `Unreachable` with no clue that trust was the cause.

    /// The root the chain above terminates at.
    const REQUIRED_ROOT: &str = "Telia Root CA v2";

    #[test]
    fn the_root_the_chat_server_chains_to_is_in_the_compiled_in_store() {
        let needle = REQUIRED_ROOT.as_bytes();
        let present = webpki_roots::TLS_SERVER_ROOTS.iter().any(|anchor| {
            anchor
                .subject
                .as_ref()
                .windows(needle.len())
                .any(|window| window == needle)
        });

        assert!(
            present,
            "`{REQUIRED_ROOT}` is no longer in webpki-roots, so deliveries to \
             the chat server would fail TLS -- and the failure arrives as \
             DeliveryFailure::Unreachable, which reads as a network problem. \
             Either the root was removed upstream, in which case the chat \
             server's certificate needs checking again, or the server has been \
             reissued under a different CA."
        );
    }

    /// A guard on the guard: if the store were empty or tiny the test above
    /// would pass vacuously on a bad build.
    #[test]
    fn the_compiled_in_store_is_populated() {
        assert!(
            webpki_roots::TLS_SERVER_ROOTS.len() > 50,
            "only {} roots compiled in -- the trust store looks wrong",
            webpki_roots::TLS_SERVER_ROOTS.len()
        );
    }
}
