// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! What happened, counted, on a listener only Prometheus reaches.
//!
//! OBSERVED THROUGH DECORATORS, not by the inbound adapter. The inbound adapter
//! sees every outcome and would be the obvious place -- but it is an adapter,
//! and so is this, and the architecture test forbids one depending on the
//! other. Wrapping the PORTS instead is better than a way around the rule: the
//! counts are then recorded whatever reaches the application, and neither the
//! domain nor the use case learns that anything is being counted.
//!
//! A SECOND LISTENER, NOT A SECOND ROUTE, and the reason is measured upstream.
//! webhook-proxy serves `/metrics` on the port its tenants must reach for the
//! claim/report hand-off, so any tenant can read every other tenant's room
//! names and delivery counts -- filed as `gc-wdt` after being done by accident
//! from a tenant namespace three times in one session. Here it is free to get
//! right: only Alertmanager needs the inbound port and only Prometheus needs
//! this one, so a `NetworkPolicy` can separate them.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use application::delivering::{DeliveryFailure, Renderer, Rooms};
use async_trait::async_trait;
use axum::Router;
use axum::routing::get;
use domain::notification::Notification;
use domain::rendering::{FallbackReason, Message};

/// Everything counted, shared by the decorators and the endpoint.
#[derive(Debug, Default)]
pub struct Counts {
    /// (template, outcome) -> count.
    renders: Mutex<BTreeMap<(String, &'static str), u64>>,
    /// (room key, outcome) -> count.
    deliveries: Mutex<BTreeMap<(String, &'static str), u64>>,
    /// Whether Unicode sanitising is wired, decided by the composition root.
    sanitiser: Mutex<bool>,
}

impl Counts {
    /// A fresh set.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Record whether the sanitising decorator was wired.
    ///
    /// A FEATURE THAT IS "SWITCHED ON" SOMEWHERE IN A VALUES FILE is a thing
    /// nobody can assert from outside the process. Reporting it as a gauge
    /// makes it checkable -- the same gap upstream's `gc-wm8` describes for a
    /// storage mode, where the warning existed only in a log line.
    pub fn sanitising(&self, enabled: bool) {
        if let Ok(mut flag) = self.sanitiser.lock() {
            *flag = enabled;
        }
    }

    fn render(&self, template: &str, outcome: &'static str) {
        if let Ok(mut counts) = self.renders.lock() {
            *counts.entry((template.to_owned(), outcome)).or_default() += 1;
        }
    }

    fn delivery(&self, room: &str, outcome: &'static str) {
        if let Ok(mut counts) = self.deliveries.lock() {
            *counts.entry((room.to_owned(), outcome)).or_default() += 1;
        }
    }

    /// The Prometheus text exposition of everything counted.
    #[must_use]
    pub fn exposition(&self) -> String {
        let mut out = String::new();

        // NO BARE UNLABELLED TOTALS on either counter. That is upstream's
        // `gc-5px`: an unlabelled total reads as a cluster-wide figure, and
        // with several replicas a scrape through a Service returns one pod's
        // share without the output admitting it. The HELP text says so, because
        // a single-endpoint scrape is the obvious thing to write.
        let _ = writeln!(
            out,
            "# HELP alert_render_render_total Renderings by template and outcome. \
             Per-process: sum by (template, outcome) across replicas."
        );
        let _ = writeln!(out, "# TYPE alert_render_render_total counter");
        if let Ok(counts) = self.renders.lock() {
            for ((template, outcome), count) in counts.iter() {
                let _ = writeln!(
                    out,
                    "alert_render_render_total{{template=\"{template}\",outcome=\"{outcome}\"}} {count}"
                );
            }
        }

        let _ = writeln!(
            out,
            "# HELP alert_render_delivery_total Deliveries by room and outcome. \
             Per-process: sum by (room, outcome) across replicas. The room is a \
             secret KEY, never an address."
        );
        let _ = writeln!(out, "# TYPE alert_render_delivery_total counter");
        if let Ok(counts) = self.deliveries.lock() {
            for ((room, outcome), count) in counts.iter() {
                let _ = writeln!(
                    out,
                    "alert_render_delivery_total{{room=\"{room}\",outcome=\"{outcome}\"}} {count}"
                );
            }
        }

        // TWO GAUGES, AND THEY CARRY NO LABELS ON PURPOSE. The no-bare-total
        // rule is about COUNTERS, where an unlabelled figure reads as an
        // aggregate across pods. These report a property of THIS process, which
        // is not an aggregate of anything -- and `namespace` and `pod` arrive
        // from the scrape, which a tenant cannot forge, so labelling them here
        // would be noise that also duplicates a stronger label.
        let _ = writeln!(
            out,
            "# HELP alert_render_sanitiser_enabled 1 when Unicode sanitising is wired in this process, 0 otherwise."
        );
        let _ = writeln!(out, "# TYPE alert_render_sanitiser_enabled gauge");
        let enabled = self.sanitiser.lock().is_ok_and(|flag| *flag);
        let _ = writeln!(out, "alert_render_sanitiser_enabled {}", u8::from(enabled));

        let _ = writeln!(
            out,
            "# HELP alert_render_build_info The version this process was built from."
        );
        let _ = writeln!(out, "# TYPE alert_render_build_info gauge");
        let _ = writeln!(
            out,
            "alert_render_build_info{{version=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION")
        );

        out
    }
}

/// The metrics endpoint, for its OWN listener.
pub fn router(counts: Arc<Counts>) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let counts = Arc::clone(&counts);
            async move { counts.exposition() }
        }),
    )
}

fn render_outcome(result: &Result<Message, FallbackReason>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(FallbackReason::TemplateMissing) => "no_template",
        Err(FallbackReason::TemplateMalformed) => "malformed_template",
        Err(FallbackReason::RenderFailed) => "render_failed",
    }
}

fn delivery_outcome(result: Result<(), DeliveryFailure>) -> &'static str {
    match result {
        Ok(()) => "delivered",
        Err(DeliveryFailure::Refused) => "refused",
        Err(DeliveryFailure::Intercepted) => "intercepted",
        Err(DeliveryFailure::Unreachable) => "unreachable",
        Err(DeliveryFailure::TimedOut) => "timed_out",
        Err(DeliveryFailure::Unaddressable) => "unaddressable",
    }
}

/// A Renderer that counts what it rendered.
pub struct Counted<T> {
    inner: T,
    counts: Arc<Counts>,
}

impl<T> Counted<T> {
    /// Wrap `inner`, counting into `counts`.
    pub fn new(inner: T, counts: Arc<Counts>) -> Self {
        Self { inner, counts }
    }
}

#[async_trait]
impl<T: Renderer> Renderer for Counted<T> {
    async fn render(
        &self,
        template_id: &str,
        notification: &Notification,
    ) -> Result<Message, FallbackReason> {
        let result = self.inner.render(template_id, notification).await;
        self.counts.render(template_id, render_outcome(&result));
        result
    }
}

#[async_trait]
impl<T: Rooms> Rooms for Counted<T> {
    async fn deliver(&self, room_key: &str, message: &Message) -> Result<(), DeliveryFailure> {
        let result = self.inner.deliver(room_key, message).await;
        self.counts.delivery(room_key, delivery_outcome(result));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{Counted, Counts, router};
    use application::delivering::{DeliveryFailure, Renderer, Rooms};
    use async_trait::async_trait;
    use domain::notification::{Alert, AlertState, Annotations, Labels, Notification};
    use domain::rendering::{FallbackReason, Message};

    struct Fixed(Result<Message, FallbackReason>);
    #[async_trait]
    impl Renderer for Fixed {
        async fn render(&self, _id: &str, _n: &Notification) -> Result<Message, FallbackReason> {
            self.0.clone()
        }
    }

    struct Answering(Result<(), DeliveryFailure>);
    #[async_trait]
    impl Rooms for Answering {
        async fn deliver(&self, _k: &str, _m: &Message) -> Result<(), DeliveryFailure> {
            self.0
        }
    }

    fn a_notification() -> Notification {
        Notification::new(vec![Alert::new(
            AlertState::Firing,
            Labels::default(),
            Annotations::default(),
        )])
        .expect("one alert")
    }

    fn a_message() -> Message {
        Message::new("<b>x</b>", "x").expect("well-formed")
    }

    /// Each fallback reason is its own series, because they are different
    /// things for a tenant to fix: a template nobody mounted, one that does not
    /// parse, and one that parses and then goes wrong.
    #[tokio::test]
    async fn every_render_outcome_is_its_own_series() {
        let counts = Counts::new();

        for (result, expected) in [
            (Ok(a_message()), "ok"),
            (Err(FallbackReason::TemplateMissing), "no_template"),
            (Err(FallbackReason::TemplateMalformed), "malformed_template"),
            (Err(FallbackReason::RenderFailed), "render_failed"),
        ] {
            let metered = Counted::new(Fixed(result), counts.clone());
            let _ = metered.render("team", &a_notification()).await;
            assert!(
                counts.exposition().contains(&format!(
                    "alert_render_render_total{{template=\"team\",outcome=\"{expected}\"}} 1"
                )),
                "{expected} is missing:\n{}",
                counts.exposition()
            );
        }
    }

    /// And each delivery outcome, including Unaddressable -- which is a
    /// configuration fault rather than a network one, and counting it with the
    /// network failures would send whoever is paged to the wrong place.
    #[tokio::test]
    async fn every_delivery_outcome_is_its_own_series() {
        let counts = Counts::new();

        for (result, expected) in [
            (Ok(()), "delivered"),
            (Err(DeliveryFailure::Refused), "refused"),
            (Err(DeliveryFailure::Intercepted), "intercepted"),
            (Err(DeliveryFailure::Unreachable), "unreachable"),
            (Err(DeliveryFailure::TimedOut), "timed_out"),
            (Err(DeliveryFailure::Unaddressable), "unaddressable"),
        ] {
            let metered = Counted::new(Answering(result), counts.clone());
            let _ = metered.deliver("team-room-url", &a_message()).await;
            assert!(
                counts.exposition().contains(&format!(
                    "alert_render_delivery_total{{room=\"team-room-url\",outcome=\"{expected}\"}} 1"
                )),
                "{expected} is missing:\n{}",
                counts.exposition()
            );
        }
    }

    /// NO BARE UNLABELLED TOTAL, which is upstream's gc-5px: an unlabelled
    /// total reads as a cluster-wide figure, and with several replicas a scrape
    /// through a Service returns one pod's share without saying so. Asserted on
    /// the output rather than trusted to the writer.
    #[tokio::test]
    async fn no_counter_is_exposed_without_labels() {
        let counts = Counts::new();
        let metered = Counted::new(Answering(Ok(())), counts.clone());
        let _ = metered.deliver("team-room-url", &a_message()).await;

        for line in counts.exposition().lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let name = line.split_whitespace().next().unwrap_or_default();
            if name.ends_with("_total") {
                assert!(
                    name.contains('{'),
                    "a bare total reads as an aggregate across pods: {line}"
                );
            }
        }
    }

    /// The HELP text has to say a sum is required, because a single-endpoint
    /// scrape through a Service is the obvious thing to write and it is wrong.
    #[tokio::test]
    async fn the_help_text_says_a_sum_across_replicas_is_required() {
        let exposition = Counts::new().exposition();
        let helps: Vec<&str> = exposition
            .lines()
            .filter(|line| line.starts_with("# HELP") && line.contains("_total"))
            .collect();

        assert_eq!(helps.len(), 2, "{helps:?}");
        for help in helps {
            assert!(help.contains("sum by"), "no aggregation note: {help}");
        }
    }

    /// A feature that is "switched on" somewhere in a values file is a thing
    /// nobody can assert from outside the process.
    #[tokio::test]
    async fn the_sanitiser_gauge_reports_what_the_root_wired() {
        let counts = Counts::new();
        assert!(
            counts
                .exposition()
                .contains("alert_render_sanitiser_enabled 0")
        );

        counts.sanitising(true);
        assert!(
            counts
                .exposition()
                .contains("alert_render_sanitiser_enabled 1")
        );
    }

    #[tokio::test]
    async fn the_build_version_is_reported() {
        assert!(
            Counts::new()
                .exposition()
                .contains("alert_render_build_info{version=\"")
        );
    }

    /// A room KEY is safe to expose -- it is a name. An ADDRESS is the whole
    /// credential, and nothing in this crate ever sees one: the decorator is
    /// handed the key, and resolving it happens below. Asserted so that a later
    /// change which started labelling by address would fail here.
    #[tokio::test]
    async fn no_series_carries_a_room_address() {
        let counts = Counts::new();
        let metered = Counted::new(Answering(Ok(())), counts.clone());
        let _ = metered.deliver("team-room-url", &a_message()).await;

        let exposition = counts.exposition();
        assert!(!exposition.contains("http://"), "{exposition}");
        assert!(!exposition.contains("https://"), "{exposition}");
    }

    /// THE ENDPOINT IS NOT ON THE INBOUND ROUTER. This crate exposes a router
    /// of its own and nothing else, so there is no way to mount it on the
    /// listener Alertmanager reaches -- which is how upstream's gc-wdt
    /// happened, where every tenant could read every other tenant's room names.
    #[tokio::test]
    async fn the_metrics_router_serves_metrics_and_nothing_else() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let app = router(Counts::new());

        let metrics = app
            .clone()
            .oneshot(Request::get("/metrics").body(Body::empty()).expect("req"))
            .await
            .expect("res");
        assert_eq!(metrics.status(), StatusCode::OK);

        // The inbound paths are not here. A binding's listener on this router
        // would mean Alertmanager could post to the metrics port.
        let notify = app
            .oneshot(
                Request::post("/notify/team")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("res");
        assert_eq!(notify.status(), StatusCode::NOT_FOUND);
    }
}
