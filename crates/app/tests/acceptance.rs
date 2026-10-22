// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! The real binary, real directories, a real socket, and no test doubles.
//!
//! WHY THIS LEVEL EXISTS AT ALL. Every adapter has unit tests with doubles, and
//! they all pass whether or not the wiring is right -- a port satisfied by the
//! wrong adapter compiles and does nothing. These tests start the process the
//! way a container does and are the only place "the wiring is wrong" is
//! detectable.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// A stub room: answers `{"ok":true}` and records what it was handed.
struct StubRoom {
    address: String,
    received: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

async fn stub_room() -> StubRoom {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&received);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut scratch = vec![0_u8; 65536];
            let read = socket.read(&mut scratch).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&scratch[..read]).into_owned();
            seen.lock().expect("not poisoned").push(request);
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
                )
                .await;
            let _ = socket.flush().await;
        }
    });

    StubRoom {
        address: format!("http://127.0.0.1:{port}/room"),
        received,
    }
}

#[derive(Debug)]
struct Running {
    child: Child,
    address: String,
}

impl Drop for Running {
    fn drop(&mut self) {
        // Each test gets its own process, and it listens on an ephemeral port,
        // so leaving one behind would leak a server for the rest of the run.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A directory tree the way a container would mount one.
fn mounted(
    bindings: &str,
    templates: &[(&str, &str)],
    rooms: &[(&str, &str)],
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("bindings.yaml"), bindings).expect("bindings");
    std::fs::create_dir(dir.path().join("templates")).expect("templates dir");
    for (name, source) in templates {
        std::fs::write(dir.path().join("templates").join(name), source).expect("template");
    }
    std::fs::create_dir(dir.path().join("rooms")).expect("rooms dir");
    for (key, address) in rooms {
        std::fs::write(dir.path().join("rooms").join(key), address).expect("room");
    }
    dir
}

/// Start the binary against `dir`, returning it and the address it announced.
fn start(dir: &tempfile::TempDir) -> Result<Running, String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_alert-render"))
        .env_clear()
        .env("ALERT_RENDER_LISTEN", "127.0.0.1:0")
        .env("ALERT_RENDER_METRICS_LISTEN", "127.0.0.1:0")
        .env("ALERT_RENDER_BINDINGS", dir.path().join("bindings.yaml"))
        .env("ALERT_RENDER_TEMPLATES", dir.path().join("templates"))
        .env("ALERT_RENDER_ROOMS", dir.path().join("rooms"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");

    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = BufReader::new(stdout).lines();

    match lines.next() {
        Some(Ok(line)) if line.starts_with("listening on ") => {
            let address = line.trim_start_matches("listening on ").trim().to_owned();
            Ok(Running { child, address })
        }
        // It exited, or said something else. READ ALL OF STDERR rather than one
        // line: a process that refuses says the interesting thing somewhere in
        // the middle, which is a lesson webhook-proxy's harness had to learn
        // the second time its startup output changed.
        other => {
            let mut why = String::new();
            if let Some(stderr) = child.stderr.take() {
                use std::io::Read as _;
                let _ = BufReader::new(stderr).read_to_string(&mut why);
            }
            let _ = child.wait();
            Err(format!(
                "first stdout line was {other:?}; stderr said: {why}"
            ))
        }
    }
}

const BODY: &str = r#"{"alerts":[{"status":"firing","labels":{"alertname":"DiskFull","severity":"critical"},"annotations":{"summary":"disk is full"}}]}"#;

fn bindings_for(room_key: &str) -> String {
    format!("bindings:\n  - listener: /notify/team\n    template: team\n    room: {room_key}\n")
}

fn templates() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "team.html",
            "<b>{{ notification.alerts[0].labels.alertname }}</b> {{ notification.alerts[0].annotations.summary }}",
        ),
        (
            "team.txt",
            "{{ notification.alerts[0].labels.alertname }} {{ notification.alerts[0].annotations.summary }}",
        ),
    ]
}

#[tokio::test]
async fn a_notification_is_rendered_and_delivered_end_to_end() {
    let room = stub_room().await;
    let dir = mounted(
        &bindings_for("team-room-url"),
        &templates(),
        &[("team-room-url", &room.address)],
    );
    let running = start(&dir).expect("it starts");

    let answer = reqwest::Client::new()
        .post(format!("http://{}/notify/team", running.address))
        .header("content-type", "application/json")
        .body(BODY)
        .send()
        .await
        .expect("a response");

    assert_eq!(answer.status(), 200);

    let seen = room.received.lock().expect("not poisoned").clone();
    assert_eq!(seen.len(), 1, "the room was handed {} requests", seen.len());
    // The RENDERED message, through the real template, escaped in html and raw
    // in text -- which is every adapter in the chain having been wired to the
    // right port.
    assert!(
        seen[0].contains("<b>DiskFull</b> disk is full"),
        "{}",
        seen[0]
    );
    assert!(seen[0].contains("DiskFull disk is full"), "{}", seen[0]);
}

/// A malformed config EXITS. There is nothing to serve, so a process that
/// listened anyway would answer 404 to everything while looking healthy.
#[tokio::test]
async fn a_malformed_config_refuses_to_start_and_names_the_file() {
    let dir = mounted("bindings: [ not valid: yaml\n", &templates(), &[]);

    let refused = start(&dir).expect_err("it must not listen");

    assert!(
        refused.contains("bindings.yaml"),
        "the complaint does not name the file: {refused}"
    );
}

/// A MISSING ROOM FILE DOES NOT STOP IT. The binding is valid, its path should
/// be served, the secret may be rendered a moment later -- and one unmounted
/// room must not take down every other binding in the deployment.
#[tokio::test]
async fn a_room_key_with_no_file_still_lets_the_process_start() {
    let dir = mounted(&bindings_for("not-mounted-yet"), &templates(), &[]);

    let running = start(&dir).expect("it starts anyway");

    // And the delivery fails as a 5xx, so Alertmanager retries -- by which
    // time the secret may be there.
    let answer = reqwest::Client::new()
        .post(format!("http://{}/notify/team", running.address))
        .header("content-type", "application/json")
        .body(BODY)
        .send()
        .await
        .expect("a response");

    assert!(
        answer.status().is_server_error(),
        "got {} -- a 4xx would tell Alertmanager to stop trying",
        answer.status()
    );
}

/// Metrics are on their OWN listener, and the inbound one does not serve them.
/// That is upstream's gc-wdt: /metrics on the port a tenant must reach meant
/// every tenant could read every other tenant's room names.
#[tokio::test]
async fn metrics_are_not_on_the_inbound_listener() {
    let room = stub_room().await;
    let dir = mounted(
        &bindings_for("team-room-url"),
        &templates(),
        &[("team-room-url", &room.address)],
    );
    let running = start(&dir).expect("it starts");

    let on_inbound = reqwest::Client::new()
        .get(format!("http://{}/metrics", running.address))
        .send()
        .await
        .expect("a response");

    assert_eq!(
        on_inbound.status(),
        404,
        "metrics answered on the inbound port"
    );
}
