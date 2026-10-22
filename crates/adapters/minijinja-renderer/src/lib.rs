// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Rendering a Notification with a tenant's template.
//!
//! TWO TEMPLATES PER BINDING, `<id>.html` and `<id>.txt`, and this is what the
//! engine was chosen for. A chat room is told `{html, text}`: the html body
//! must be escaped and the text body must not -- escaping plain text shows
//! `&lt;` to a reader whose client prefers that field. minijinja picks
//! auto-escaping from the template NAME, so the `.html` one gets it and the
//! `.txt` one does not, with no per-call flag to forget. One template producing
//! both bodies would put that choice in the tenant's hands.
//!
//! A TEMPLATE IS UNTRUSTED INPUT. It is authored in a tenant's gitops
//! repository and loaded at runtime, so it belongs in the same category as a
//! request body. Two settings follow from that and neither is a default:
//! a FUEL BUDGET, so a runaway loop is stopped by the engine rather than by a
//! supervisor; and STRICT undefined behaviour, so a typo is an error rather
//! than a blank.

use application::delivering::Renderer;
use async_trait::async_trait;
use domain::notification::{AlertState, Notification};
use domain::rendering::{FallbackReason, Message};
use minijinja::{Environment, UndefinedBehavior};
use std::collections::BTreeMap;

/// How much work one template may do before it is stopped.
///
/// Fuel counts engine operations rather than time, so it is deterministic --
/// the same template costs the same on a busy node as on an idle one, which a
/// timeout is not. The number is generous for any reasonable template over a
/// group of alerts and nowhere near enough for an unbounded loop.
const FUEL: u64 = 500_000;

/// Renders with templates held in memory.
pub struct MinijinjaRenderer {
    env: Environment<'static>,
    sources: BTreeMap<String, String>,
}

impl MinijinjaRenderer {
    /// A renderer over `sources`, keyed by file name such as `team.html`.
    #[must_use]
    pub fn holding(sources: BTreeMap<String, String>) -> Self {
        let mut env = Environment::new();
        // A typo must not render empty. `{{ alert.labels.sevrity }}` producing a
        // blank sends something to a room that looks like an alert with no
        // severity, and nobody investigates that.
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        env.set_fuel(Some(FUEL));
        Self { env, sources }
    }
}

/// The word for a state, for a template that wants to print it.
fn state_word(state: AlertState) -> &'static str {
    match state {
        AlertState::Firing => "firing",
        AlertState::Resolved => "resolved",
    }
}

/// The surface a template may read, mapped from the domain.
///
/// A DELIBERATE VIEW, NOT THE DOMAIN TYPES (ar-1kx.3). Whatever a template can
/// reach becomes an interface tenants write against, so handing them the domain
/// would make every later rename of a field a breaking change for every
/// template. This function is the contract, and it is the only place that has
/// to change when the domain moves.
fn surface(notification: &Notification) -> minijinja::Value {
    let alerts: Vec<minijinja::Value> = notification
        .alerts()
        .iter()
        .map(|alert| {
            // `from_pairs` rather than serde: minijinja 3 does not enable serde
            // by default, and a map of names to strings needs no serialiser.
            let labels = minijinja::Value::from_pairs(alert.labels().iter());
            let annotations = minijinja::Value::from_pairs(alert.annotations().iter());
            minijinja::context! {
                // Both, and for different jobs: `state` is printed, `firing` is
                // branched on. A template wanting the word in its output should
                // not have to write a conditional to get it.
                state => state_word(alert.state()),
                firing => alert.state() == AlertState::Firing,
                labels => labels,
                annotations => annotations,
            }
        })
        .collect();

    minijinja::context! {
        notification => minijinja::context! {
            count => alerts.len(),
            alerts => alerts,
        },
    }
}

/// Why the templates could not be read.
#[derive(Debug, thiserror::Error)]
pub enum TemplatesUnreadable {
    /// The directory is not there, or cannot be listed.
    #[error("the template directory {path} could not be read")]
    Unreadable {
        /// Where it was looked for.
        path: std::path::PathBuf,
    },
}

impl MinijinjaRenderer {
    /// A renderer over every `.html` and `.txt` file in `directory`.
    ///
    /// READ ONCE AT STARTUP, like the Bindings and unlike a Room address:
    /// templates change through gitops, so a change should roll the pods --
    /// which a `ConfigMap` whose name carries a content hash gives for free.
    ///
    /// Other extensions are IGNORED rather than refused. A `ConfigMap` mount
    /// carries `..data` symlinks and dotfiles of Kubernetes' own, and refusing
    /// the directory because of them would mean refusing every real mount.
    ///
    /// # Errors
    ///
    /// [`TemplatesUnreadable`] if the directory cannot be listed.
    pub fn holding_directory(
        directory: impl AsRef<std::path::Path>,
    ) -> Result<Self, TemplatesUnreadable> {
        let directory = directory.as_ref();
        let entries =
            std::fs::read_dir(directory).map_err(|_| TemplatesUnreadable::Unreadable {
                path: directory.to_path_buf(),
            })?;

        let mut sources = BTreeMap::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // CASE-SENSITIVE ON PURPOSE, against clippy's advice here. The
            // suggestion is to compare the extension case-insensitively, and
            // that would be WRONG: minijinja selects auto-escaping from the
            // template name and its callback matches `.html` in lower case. A
            // file called `team.HTML` accepted here would therefore be rendered
            // with escaping OFF -- a silent hole in the one protection the html
            // body has. Refusing to load it is the safe answer.
            #[allow(
                clippy::case_sensitive_file_extension_comparisons,
                reason = "matching minijinja's own lower-case autoescape callback; see above"
            )]
            if !(name.ends_with(".html") || name.ends_with(".txt")) {
                continue;
            }
            if let Ok(source) = std::fs::read_to_string(entry.path()) {
                sources.insert(name, source);
            }
        }

        Ok(Self::holding(sources))
    }
}

#[async_trait]
impl Renderer for MinijinjaRenderer {
    async fn render(
        &self,
        template_id: &str,
        notification: &Notification,
    ) -> Result<Message, FallbackReason> {
        let context = surface(notification);
        let html = self.body(&format!("{template_id}.html"), &context)?;
        let text = self.body(&format!("{template_id}.txt"), &context)?;

        // A template may legally render to nothing -- `{% if severity ==
        // 'critical' %}` with a warning alert. Message::new refuses a blank
        // body, and that refusal is a RENDER failure rather than a panic: the
        // tenant then gets the fallback and can see why.
        Message::new(&html, &text).map_err(|_| FallbackReason::RenderFailed)
    }
}

impl MinijinjaRenderer {
    /// One body, by template file name.
    fn body(&self, name: &str, context: &minijinja::Value) -> Result<String, FallbackReason> {
        let source = self
            .sources
            .get(name)
            .ok_or(FallbackReason::TemplateMissing)?;

        // NAMED, because the name is what selects auto-escaping: `.html` is
        // escaped and `.txt` is not. Compiled per render rather than once at
        // startup, which is what makes a malformed template one tenant's
        // fallback instead of a deployment that will not start.
        let template = self
            .env
            .template_from_named_str(name, source)
            .map_err(|_| FallbackReason::TemplateMalformed)?;

        template
            .render(context)
            .map_err(|_| FallbackReason::RenderFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::MinijinjaRenderer;
    use application::delivering::Renderer;
    use domain::notification::{Alert, AlertState, Annotations, Labels, Notification};
    use domain::rendering::FallbackReason;
    use std::collections::BTreeMap;

    fn sources(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn one_alert(labels: Labels) -> Notification {
        Notification::new(vec![Alert::new(
            AlertState::Firing,
            labels,
            Annotations::from([("summary", "disk is full")]),
        )])
        .expect("one alert is a notification")
    }

    /// THE ASYMMETRY THE ENGINE WAS CHOSEN FOR. A label is tenant data and can
    /// contain anything. In the html body it must arrive escaped; in the text
    /// body it must NOT, because escaping plain text shows `&lt;` to a reader
    /// whose client prefers that field. Neither template says anything about
    /// escaping -- the file extension does.
    #[tokio::test]
    async fn the_same_value_is_escaped_in_html_and_raw_in_text() {
        let renderer = MinijinjaRenderer::holding(sources(&[
            ("t.html", "{{ notification.alerts[0].annotations.summary }}"),
            ("t.txt", "{{ notification.alerts[0].annotations.summary }}"),
        ]));
        let notification = Notification::new(vec![Alert::new(
            AlertState::Firing,
            Labels::from([("alertname", "X")]),
            Annotations::from([("summary", "<script>alert(1)</script> 5 > 3 & \"q\"")]),
        )])
        .expect("one alert");

        let message = renderer.render("t", &notification).await.expect("renders");

        assert!(!message.html().contains("<script>"), "{}", message.html());
        assert!(
            message.html().contains("&lt;script&gt;"),
            "{}",
            message.html()
        );
        assert!(message.html().contains("&amp;"), "{}", message.html());
        assert_eq!(
            message.text(),
            "<script>alert(1)</script> 5 > 3 & \"q\"",
            "the text body must be the value as it was"
        );
    }

    /// Each failure is a different `FallbackReason`, because they are different
    /// things for a tenant to fix: a template that is not mounted, one that
    /// does not parse, and one that parses and then goes wrong.
    #[tokio::test]
    async fn a_template_that_is_not_mounted_is_missing_not_malformed() {
        let renderer = MinijinjaRenderer::holding(sources(&[("other.html", "x")]));
        assert_eq!(
            renderer.render("team", &one_alert(Labels::default())).await,
            Err(FallbackReason::TemplateMissing)
        );

        // And having only ONE of the pair is still missing -- a binding needs
        // both bodies, and half a Message is not a Message.
        let half = MinijinjaRenderer::holding(sources(&[("team.html", "x")]));
        assert_eq!(
            half.render("team", &one_alert(Labels::default())).await,
            Err(FallbackReason::TemplateMissing)
        );
    }

    #[tokio::test]
    async fn a_template_that_does_not_parse_is_malformed() {
        let renderer = MinijinjaRenderer::holding(sources(&[
            ("t.html", "{% for a in notification.alerts %}unclosed"),
            ("t.txt", "fine"),
        ]));
        assert_eq!(
            renderer.render("t", &one_alert(Labels::default())).await,
            Err(FallbackReason::TemplateMalformed)
        );
    }

    /// A TYPO MUST NOT RENDER EMPTY (ar-1kx.3). Strict undefined behaviour is
    /// what turns `sevrity` into a failure the tenant sees as a fallback,
    /// rather than a blank that reaches a room looking like an alert with no
    /// severity -- which nobody investigates.
    #[tokio::test]
    async fn a_misspelled_attribute_fails_rather_than_rendering_blank() {
        let renderer = MinijinjaRenderer::holding(sources(&[
            // SURROUNDED BY REAL CONTENT, deliberately. An earlier version of
            // this test rendered only the typo, so a lenient engine would have
            // produced an empty body, Message::new would have refused it, and
            // the result would have been RenderFailed either way -- the test
            // would have passed without strict undefined doing anything. With
            // "severity is " around it, a lenient engine renders a perfectly
            // valid Message and this fails.
            (
                "t.html",
                "severity is {{ notification.alerts[0].labels.sevrity }}",
            ),
            (
                "t.txt",
                "severity is {{ notification.alerts[0].labels.sevrity }}",
            ),
        ]));
        assert_eq!(
            renderer
                .render("t", &one_alert(Labels::from([("severity", "critical")])))
                .await,
            Err(FallbackReason::RenderFailed)
        );
    }

    /// A template is untrusted input, so an unbounded loop must be stopped by
    /// the engine. Per-tenant deployment means a pinned renderer is one team's
    /// alerts stopped rather than everyone's -- which is why a fuel budget is
    /// sufficient here rather than merely helpful.
    #[tokio::test]
    async fn an_unbounded_loop_is_stopped_by_the_fuel_budget() {
        // NESTED SMALL RANGES, and the nesting is the whole point. An earlier
        // version of this test used `range(2000000)`, which passed with the
        // fuel budget REMOVED -- because minijinja's `range` has a cap of its
        // own and answers "range has too many elements". The test proved the
        // cap existed and said nothing about fuel. Two ranges of 1000 are both
        // well inside that cap and together are a million iterations, which is
        // far past the budget.
        let runaway = MinijinjaRenderer::holding(sources(&[
            (
                "t.html",
                "{% for i in range(1000) %}{% for j in range(1000) %}{{ j }}{% endfor %}{% endfor %}",
            ),
            ("t.txt", "x"),
        ]));
        let began = std::time::Instant::now();
        assert_eq!(
            runaway.render("t", &one_alert(Labels::default())).await,
            Err(FallbackReason::RenderFailed)
        );
        assert!(
            began.elapsed() < std::time::Duration::from_secs(5),
            "fuel did not bound it: {:?}",
            began.elapsed()
        );

        let ordinary = MinijinjaRenderer::holding(sources(&[
            ("t.html", "{% for i in range(10) %}{{ i }}{% endfor %}"),
            ("t.txt", "x"),
        ]));
        assert!(
            ordinary
                .render("t", &one_alert(Labels::default()))
                .await
                .is_ok(),
            "the budget is too small for an ordinary loop"
        );
    }

    /// THE CASE MOST LIKELY TO BE MISSED. A template may legally render to
    /// nothing, and `Message::new` refuses a blank body -- so this must be a
    /// `RenderFailed` and the tenant's fallback, not an unwrap on an `Err`.
    #[tokio::test]
    async fn a_template_that_renders_nothing_fails_rather_than_panicking() {
        let renderer = MinijinjaRenderer::holding(sources(&[
            (
                "t.html",
                "{% if notification.alerts[0].labels.severity is defined %}x{% endif %}",
            ),
            ("t.txt", "y"),
        ]));
        assert_eq!(
            renderer.render("t", &one_alert(Labels::default())).await,
            Err(FallbackReason::RenderFailed)
        );
    }

    /// The surface decided in ar-1kx.3, exercised through a template rather
    /// than asserted as a struct: a template is the only consumer, so a
    /// template is the only honest test of it.
    #[tokio::test]
    async fn a_template_renders_both_bodies_from_the_declared_surface() {
        let renderer = MinijinjaRenderer::holding(sources(&[
            (
                "team.html",
                "<b>{{ notification.count }}</b> {% for a in notification.alerts %}{{ a.labels.alertname }} is {{ a.state }}{% endfor %}",
            ),
            (
                "team.txt",
                "{{ notification.count }} {% for a in notification.alerts %}{{ a.labels.alertname }} is {{ a.state }}{% if a.firing %}!{% endif %}{% endfor %}",
            ),
        ]));

        let message = renderer
            .render(
                "team",
                &one_alert(Labels::from([("alertname", "DiskFull")])),
            )
            .await
            .expect("a well-formed template");

        assert_eq!(message.html(), "<b>1</b> DiskFull is firing");
        assert_eq!(message.text(), "1 DiskFull is firing!");
    }
}
