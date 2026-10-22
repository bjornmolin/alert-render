// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! The composition root: the only place that knows which adapter satisfies
//! which port.
//!
//! WHAT FAILS AT STARTUP AND WHAT FAILS PER DELIVERY is the decision this file
//! makes, and the two halves look contradictory until you see why:
//!
//! - A MALFORMED CONFIG EXITS. There is nothing to serve, the listener paths
//!   are unknown, and a process that listened anyway would answer 404 to
//!   everything while looking healthy.
//! - A MISSING ROOM FILE DOES NOT. The binding is valid, its path should be
//!   served, and the room may be mounted a moment later by a secret agent that
//!   has not finished rendering it. Failing here would also mean one unmounted
//!   room takes down every other binding in the same deployment.
//!
//! STDOUT IS A PROTOCOL: its first line is the listen address, which an
//! acceptance harness and an operator both read. Everything else goes to
//! stderr. webhook-proxy learned this expensively -- a startup warning printed
//! on stdout broke every acceptance test in its suite at once.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;

use application::delivering::Courier;
use http_rooms::{Acceptance, HttpRooms};
use metrics::{Counted, Counts};
use minijinja_renderer::MinijinjaRenderer;
use mounted_rooms::MountedRooms;
use sanitising_rooms::Sanitising;

/// Where everything is, read from the environment.
struct Settings {
    listen: String,
    metrics_listen: String,
    bindings: String,
    templates: String,
    rooms: String,
    sanitise: bool,
    acceptance: Acceptance,
}

fn setting(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_owned())
}

impl Settings {
    fn from_environment() -> Self {
        Self {
            listen: setting("ALERT_RENDER_LISTEN", "0.0.0.0:8080"),
            metrics_listen: setting("ALERT_RENDER_METRICS_LISTEN", "0.0.0.0:9090"),
            bindings: setting("ALERT_RENDER_BINDINGS", "/etc/alert-render/bindings.yaml"),
            templates: setting("ALERT_RENDER_TEMPLATES", "/etc/alert-render/templates"),
            rooms: setting("ALERT_RENDER_ROOMS", "/var/run/alert-render/rooms"),
            // ON BY DEFAULT. A protection that has to be switched on is one
            // that will be off somewhere nobody looked.
            sanitise: setting("ALERT_RENDER_SANITISE", "true") != "false",
            // STRICT BY DEFAULT, for the same reason: a hookshot answers
            // {"ok":true}, and a deployment that wants the lax predicate for an
            // inspector has to ask for it.
            acceptance: if setting("ALERT_RENDER_ACCEPTANCE", "hookshot-ok") == "any-success" {
                Acceptance::AnySuccess
            } else {
                Acceptance::HookshotOk
            },
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let settings = Settings::from_environment();

    // CONFIG FIRST, and it is the only thing that can stop the process.
    let bindings = match yaml_bindings::bindings_at(&settings.bindings) {
        Ok(bindings) => bindings,
        Err(why) => {
            eprintln!("alert-render cannot start: {why}");
            return ExitCode::FAILURE;
        }
    };

    let renderer = match MinijinjaRenderer::holding_directory(&settings.templates) {
        Ok(renderer) => renderer,
        Err(why) => {
            eprintln!("alert-render cannot start: {why}");
            return ExitCode::FAILURE;
        }
    };

    let counts = Counts::new();
    counts.sanitising(settings.sanitise);

    // The room directory is NOT checked here. A key with no file is a
    // per-delivery failure (`Unaddressable`), because the secret may arrive a
    // moment later and because one unmounted room must not take down the rest.
    let directory = MountedRooms::at(&settings.rooms);
    let delivering = match HttpRooms::new(directory, settings.acceptance, BTreeMap::new()) {
        Ok(rooms) => rooms,
        Err(why) => {
            eprintln!("alert-render cannot start: no HTTP client: {why}");
            return ExitCode::FAILURE;
        }
    };

    // THE ORDER OF THE WRAPPERS MATTERS. Sanitising goes INSIDE the counter, so
    // what is counted is what was actually delivered -- and the counter is the
    // outermost, so a sanitising decorator that ever failed would still be
    // visible in the delivery series.
    let rooms = Counted::new(Sanitising::new(delivering), Arc::clone(&counts));
    let courier = Courier::new(Counted::new(renderer, Arc::clone(&counts)), rooms);

    let inbound = inbound_http::router(courier, bindings);
    let observable = metrics::router(Arc::clone(&counts));

    let Ok(inbound_socket) = tokio::net::TcpListener::bind(&settings.listen).await else {
        eprintln!("alert-render cannot listen on {}", settings.listen);
        return ExitCode::FAILURE;
    };
    let Ok(metrics_socket) = tokio::net::TcpListener::bind(&settings.metrics_listen).await else {
        eprintln!(
            "alert-render cannot listen on {} for metrics",
            settings.metrics_listen
        );
        return ExitCode::FAILURE;
    };

    let Ok(address) = inbound_socket.local_addr() else {
        eprintln!("alert-render cannot read back its own address");
        return ExitCode::FAILURE;
    };

    // THE FIRST LINE OF STDOUT, and nothing before it.
    println!("listening on {address}");

    let metrics_server = tokio::spawn(async move {
        let _ = axum_serve(metrics_socket, observable).await;
    });
    let inbound_server = tokio::spawn(async move {
        let _ = axum_serve(inbound_socket, inbound).await;
    });

    let _ = tokio::try_join!(inbound_server, metrics_server);
    ExitCode::SUCCESS
}

/// Serve `router` on `socket`.
async fn axum_serve(
    socket: tokio::net::TcpListener,
    router: axum::Router,
) -> Result<(), std::io::Error> {
    axum::serve(socket, router).await
}
