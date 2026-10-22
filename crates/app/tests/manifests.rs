// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Tests over `deploy/alert-render/`, the reference set a tenant copies.
//!
//! WHY MANIFESTS GET TESTS AT ALL. They name things the code decides -- the
//! environment variables it reads, the ports it listens on -- and nothing
//! otherwise connects the two. A rename in `main.rs` would leave a Deployment
//! that still applies cleanly, starts, and then reads a variable nobody sets.
//! These tests make that a build failure instead of a deployment that comes up
//! answering 404 to everything.

use serde_yaml_ng::Value;

fn deploy(name: &str) -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/alert-render/").to_owned() + name;
    let text = std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{path}: {why}"));
    serde_yaml_ng::from_str(&text).unwrap_or_else(|why| panic!("{path} is not YAML: {why}"))
}

fn container(deployment: &Value) -> &Value {
    &deployment["spec"]["template"]["spec"]["containers"][0]
}

/// Every variable `main.rs` reads must be set here, or the default it falls
/// back to is the one in production and nobody chose it deliberately.
///
/// THE TEST THAT CATCHES A RENAME. `main.rs` is the source of truth for the
/// names, so they are extracted from it rather than listed again here -- a
/// second list would drift, which is the problem this is meant to solve.
#[test]
fn the_deployment_sets_every_variable_the_binary_reads() {
    let source = include_str!("../src/main.rs");
    let mut wanted: Vec<&str> = source
        .match_indices("ALERT_RENDER_")
        .map(|(at, _)| {
            let rest = &source[at..];
            let end = rest
                .find(|c: char| !(c.is_ascii_uppercase() || c == '_'))
                .unwrap_or(rest.len());
            &rest[..end]
        })
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    assert!(
        wanted.len() >= 7,
        "the extraction found only {wanted:?} -- it has stopped working"
    );

    let deployment = deploy("deployment.yaml");
    let env = container(&deployment)["env"]
        .as_sequence()
        .expect("the container sets env")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect::<Vec<&str>>();

    for name in wanted {
        assert!(
            env.contains(&name),
            "{name} is read by main.rs and not set in the Deployment, so its \
             default would be whatever the source says and nobody chose it"
        );
    }
}

/// The ports the Service exposes must be the ports the container opens. A
/// mismatch applies cleanly and then nothing can reach the pod.
#[test]
fn the_service_and_the_container_agree_about_ports() {
    let deployment = deploy("deployment.yaml");
    let service = deploy("service.yaml");

    let opened: Vec<(&str, u64)> = container(&deployment)["ports"]
        .as_sequence()
        .expect("container ports")
        .iter()
        .map(|p| {
            (
                p["name"].as_str().expect("a named port"),
                p["containerPort"].as_u64().expect("a number"),
            )
        })
        .collect();

    let exposed: Vec<(&str, u64)> = service["spec"]["ports"]
        .as_sequence()
        .expect("service ports")
        .iter()
        .map(|p| {
            (
                p["targetPort"].as_str().expect("a named targetPort"),
                p["port"].as_u64().expect("a number"),
            )
        })
        .collect();

    assert_eq!(
        opened.len(),
        2,
        "two ports, inbound and metrics: {opened:?}"
    );
    for (name, _) in &exposed {
        assert!(
            opened.iter().any(|(opened_name, _)| opened_name == name),
            "the Service targets {name}, which the container does not open"
        );
    }
}

/// Each port admitted from ONE place, which is the only reason to serve them
/// separately. An ingress rule selects by port and never by path, so two
/// services on one port could not be told apart here.
#[test]
fn the_network_policy_admits_each_port_from_one_place_only() {
    let policy = deploy("networkpolicy.yaml");
    let rules = policy["spec"]["ingress"]
        .as_sequence()
        .expect("ingress rules");

    assert_eq!(rules.len(), 2, "one rule per port");
    let ports: Vec<u64> = rules
        .iter()
        .map(|rule| {
            let listed = rule["ports"].as_sequence().expect("a rule names ports");
            assert_eq!(listed.len(), 1, "a rule must name exactly one port");
            listed[0]["port"].as_u64().expect("a number")
        })
        .collect();
    assert!(ports.contains(&8080) && ports.contains(&9090), "{ports:?}");

    // The metrics rule is the narrower of the two: a pod selector as well as a
    // namespace, because only Prometheus needs it.
    let metrics = rules
        .iter()
        .find(|rule| rule["ports"][0]["port"].as_u64() == Some(9090))
        .expect("a metrics rule");
    assert!(
        !metrics["from"][0]["podSelector"].is_null(),
        "the metrics port is admitted from a whole namespace rather than from Prometheus"
    );

    assert_eq!(
        policy["spec"]["policyTypes"]
            .as_sequence()
            .expect("policyTypes")
            .len(),
        1,
        "egress is production's business and it leaves it unrestricted; see the file"
    );
}

/// Generated, so the name carries a hash of the contents and a change rolls the
/// pods. Both files are read ONCE at startup, so a change that did not roll
/// them would simply not take effect.
#[test]
fn the_bindings_and_templates_are_content_hashed_generators() {
    let kustomization = deploy("kustomization.yaml");
    let generated: Vec<&str> = kustomization["configMapGenerator"]
        .as_sequence()
        .expect("a configMapGenerator")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();

    assert!(
        generated.contains(&"alert-render-bindings"),
        "{generated:?}"
    );
    assert!(
        generated.contains(&"alert-render-templates"),
        "{generated:?}"
    );

    // And the Deployment must mount those names, or the generator renames
    // something nothing reads.
    let deployment = deploy("deployment.yaml");
    let mounted: Vec<&str> = deployment["spec"]["template"]["spec"]["volumes"]
        .as_sequence()
        .expect("volumes")
        .iter()
        .filter_map(|volume| volume["configMap"]["name"].as_str())
        .collect();
    for name in generated {
        assert!(
            mounted.contains(&name),
            "{name} is generated and not mounted"
        );
    }
}

/// THE CREDENTIAL IS A SECRET AND NEVER A `ConfigMap`. A room address is the
/// whole thing -- whoever holds it can post to that room -- and a `ConfigMap` in
/// a gitops repository is published into git history.
#[test]
fn the_room_addresses_are_a_secret() {
    let deployment = deploy("deployment.yaml");
    let rooms = deployment["spec"]["template"]["spec"]["volumes"]
        .as_sequence()
        .expect("volumes")
        .iter()
        .find(|volume| volume["name"].as_str() == Some("rooms"))
        .expect("a rooms volume");

    assert!(
        !rooms["secret"].is_null(),
        "the room addresses are not a Secret: {rooms:?}"
    );
    assert!(
        rooms["configMap"].is_null(),
        "the room addresses are a `ConfigMap`, which publishes them into git"
    );

    // And nothing in the directory a tenant copies may hold an address.
    for name in [
        "bindings.example.yaml",
        "kustomization.yaml",
        "alertmanagerconfig.yaml",
    ] {
        let text = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/alert-render/").to_owned() + name,
        )
        .expect("readable");
        for line in text.lines() {
            // The AlertmanagerConfig's own webhook URLs are in-cluster Service
            // addresses, not room addresses -- those are the one exception.
            if line.contains("alert-render.") {
                continue;
            }
            assert!(
                !line.trim_start().starts_with("room: http"),
                "{name} puts an address where a key belongs: {line}"
            );
        }
    }
}

/// MORE THAN ONE REPLICA IS CORRECT HERE, and the assertion exists because the
/// obvious "fix" by analogy with motrice/webhook-proxy would be to set it to 1.
/// That service gates replicas > 1 on naming a database, because its deliveries
/// live in an outbox and two replicas without one each hold their own. This
/// service holds nothing: Alertmanager owns the retry.
#[test]
fn the_deployment_runs_more_than_one_replica() {
    let deployment = deploy("deployment.yaml");
    let replicas = deployment["spec"]["replicas"]
        .as_u64()
        .expect("a replica count");

    assert!(
        replicas > 1,
        "this service is stateless and Alertmanager owns the retry, so one \
         replica is a needless single point of failure -- see the comment in \
         deployment.yaml before changing this"
    );
}

/// The probes must not depend on a room being reachable. A chat server hiccup
/// would otherwise take every healthy renderer out of service, and restarting
/// would not fix something that is down elsewhere.
#[test]
fn both_probes_ask_the_process_about_itself() {
    let deployment = deploy("deployment.yaml");
    let container = container(&deployment);

    for probe in ["livenessProbe", "readinessProbe"] {
        assert_eq!(
            container[probe]["httpGet"]["path"].as_str(),
            Some("/health"),
            "{probe} does not ask /health"
        );
        assert_eq!(
            container[probe]["httpGet"]["port"].as_str(),
            Some("inbound"),
            "{probe} is not on the inbound port"
        );
    }
}

/// THE EXAMPLE TEMPLATES MUST ACTUALLY RENDER, through the real engine with the
/// real settings. They are what a tenant copies, so a template that only looked
/// plausible would send every tenant who started from it straight to the
/// fallback -- and the fallback is deliberately unmistakable, so they would see
/// it and conclude the service was broken.
///
/// This also settles a question `ar-dpp` left open: whether the `default`
/// filter still works once undefined behaviour is STRICT. The examples lean on
/// it for every optional label, so if it did not, they would all fail.
#[test]
fn the_example_templates_render_a_notification() {
    use application::delivering::Renderer;
    use domain::notification::{Alert, AlertState, Annotations, Labels, Notification};
    use minijinja_renderer::MinijinjaRenderer;

    let renderer = MinijinjaRenderer::holding_directory(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/alert-render/templates"
    ))
    .expect("the example templates load");

    // Two alerts, the second MISSING `severity` and `summary` -- which is the
    // case the `default` filter is there for and the one most likely to be
    // broken.
    let notification = Notification::new(vec![
        Alert::new(
            AlertState::Firing,
            Labels::from([("alertname", "DiskFull"), ("severity", "critical")]),
            Annotations::from([("summary", "disk is full")]),
        ),
        Alert::new(
            AlertState::Resolved,
            Labels::from([("alertname", "CpuHot")]),
            Annotations::default(),
        ),
    ])
    .expect("two alerts");

    let message = tokio_test::block_on(renderer.render("team", &notification))
        .expect("the example templates render rather than falling back");

    assert!(message.html().contains("DiskFull"), "{}", message.html());
    assert!(
        message.html().contains("disk is full"),
        "{}",
        message.html()
    );
    assert!(message.html().contains("critical"), "{}", message.html());
    // The alert with no severity and no summary fell back to the defaults
    // rather than failing the whole render.
    assert!(message.html().contains("CpuHot"), "{}", message.html());
    assert!(message.html().contains("no summary"), "{}", message.html());
    assert!(
        message.html().contains("severity none"),
        "{}",
        message.html()
    );
    // Both alerts in both bodies.
    assert!(message.text().contains("DiskFull"), "{}", message.text());
    assert!(message.text().contains("CpuHot"), "{}", message.text());
}
