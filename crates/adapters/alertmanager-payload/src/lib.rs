// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Alertmanager's webhook body, mapped to a Notification.
//!
//! UNKNOWN FIELDS ARE ACCEPTED HERE, which is the exact opposite of the rule in
//! `yaml-bindings`. The two crates sit next to each other and look like they
//! should agree, so both say why they do not:
//!
//! - the CONFIG file is a format WE own, so an unknown key is a typo. Refusing
//!   it turns `tempalte:` into a startup error instead of a binding with an
//!   empty template failing at the first delivery.
//! - this PAYLOAD is a format ALERTMANAGER owns, so an unknown field is a
//!   release we do not control. Refusing it would turn their routine upgrade
//!   into our outage.
//!
//! WHAT COMES OUT IS A PAIR: the Notification, and the Alerts as the sender
//! wrote them. The fallback exists to show a human what could not be rendered,
//! so it has to show what ARRIVED -- re-serialising from our own types would
//! omit every field this service does not model, and that is precisely the
//! field a reader might need.

use domain::notification::{
    Alert, AlertState, Annotations, Labels, MalformedNotification, Notification,
};

/// Why a body is not a Notification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotAPayload {
    /// It is not JSON, or not a JSON object.
    #[error("the body is not a JSON object")]
    NotAnObject,
    /// It has no `alerts` array.
    #[error("the body has no `alerts` array")]
    NoAlertsArray,
    /// An alert's `status` is not one Alertmanager sends.
    #[error("an alert has a status that is neither firing nor resolved")]
    UnknownStatus,
    /// The domain refused the result.
    #[error("the alerts are not a usable notification: {0:?}")]
    Unusable(MalformedNotification),
}

/// What arrived: the Notification, and each Alert as the sender wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrived {
    /// The parsed Notification.
    pub notification: Notification,
    /// One serialised Alert per element, for the fallback.
    ///
    /// FIELD ORDER IS NOT PRESERVED -- these are re-serialised from the parsed
    /// value, so every field survives and their order becomes alphabetical.
    /// Byte-exactness would mean slicing the original body, which is a lot of
    /// machinery for a reader who finds alphabetical order easier anyway.
    pub as_sent: Vec<String>,
}

/// The name/value pairs under `key`, ignoring anything that is not a string.
///
/// A non-string value is SKIPPED rather than refused. Alertmanager's labels and
/// annotations are strings, so this should not arise -- but refusing the whole
/// notification because one annotation arrived as a number would lose every
/// other alert in the group, which is a worse answer than dropping the one
/// value nobody can render anyway.
fn pairs<'a>(alert: &'a serde_json::Value, key: &str) -> Vec<(&'a str, &'a str)> {
    alert
        .get(key)
        .and_then(serde_json::Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| value.as_str().map(|value| (name.as_str(), value)))
                .collect()
        })
        .unwrap_or_default()
}

/// What arrived in `body`.
///
/// # Errors
///
/// [`NotAPayload`], distinguishing a body that is not an object, one with no
/// `alerts` array, an alert whose status is not one Alertmanager sends, and a
/// result the domain refuses.
pub fn arrived(body: &[u8]) -> Result<Arrived, NotAPayload> {
    let parsed: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| NotAPayload::NotAnObject)?;
    if !parsed.is_object() {
        return Err(NotAPayload::NotAnObject);
    }

    let sent = parsed
        .get("alerts")
        .and_then(serde_json::Value::as_array)
        .ok_or(NotAPayload::NoAlertsArray)?;

    let mut alerts = Vec::with_capacity(sent.len());
    let mut as_sent = Vec::with_capacity(sent.len());

    for one in sent {
        let state: AlertState = one
            .get("status")
            .and_then(serde_json::Value::as_str)
            .ok_or(NotAPayload::UnknownStatus)?
            .parse()
            .map_err(|_| NotAPayload::UnknownStatus)?;

        alerts.push(Alert::new(
            state,
            Labels::from_pairs(pairs(one, "labels")),
            Annotations::from_pairs(pairs(one, "annotations")),
        ));

        // AS SENT, so the fallback can show what arrived -- `fingerprint`,
        // `startsAt`, `generatorURL` and anything a later Alertmanager adds,
        // none of which this service models.
        as_sent.push(one.to_string());
    }

    let notification = Notification::new(alerts).map_err(NotAPayload::Unusable)?;
    Ok(Arrived {
        notification,
        as_sent,
    })
}

#[cfg(test)]
mod tests {
    use super::{NotAPayload, arrived};
    use domain::notification::{Alert, AlertState};

    /// A payload shaped like the real thing, including the fields this service
    /// does not model -- which is most of them.
    const REAL: &str = r#"{
      "version": "4",
      "groupKey": "{}:{alertname=\"DiskFull\"}",
      "truncatedAlerts": 0,
      "status": "firing",
      "receiver": "alert-render-team",
      "groupLabels": {"alertname": "DiskFull"},
      "commonLabels": {"alertname": "DiskFull", "severity": "critical"},
      "commonAnnotations": {},
      "externalURL": "http://alertmanager.monitoring:9093",
      "alerts": [
        {
          "status": "firing",
          "labels": {"alertname": "DiskFull", "severity": "critical"},
          "annotations": {"summary": "disk is full"},
          "startsAt": "2026-10-22T08:00:00.000Z",
          "endsAt": "0001-01-01T00:00:00Z",
          "generatorURL": "http://prometheus:9090/graph?g0.expr=up",
          "fingerprint": "a1b2c3d4e5f6"
        },
        {
          "status": "resolved",
          "labels": {"alertname": "CpuHot"},
          "annotations": {},
          "startsAt": "2026-10-22T07:00:00.000Z",
          "endsAt": "2026-10-22T07:30:00.000Z",
          "generatorURL": "http://prometheus:9090/graph",
          "fingerprint": "f6e5d4c3b2a1"
        }
      ]
    }"#;

    /// THE OPPOSITE RULE TO `yaml-bindings`, and the reason is the ownership of
    /// the format. An unknown field here is an Alertmanager release we do not
    /// control; refusing it would turn their routine upgrade into our outage.
    /// An unknown key in the CONFIG is our own typo, and that one is refused.
    #[test]
    fn a_field_alertmanager_adds_later_is_accepted_not_refused() {
        let future = r#"{
          "version": "5",
          "somethingNobodyHasInventedYet": {"nested": [1, 2, 3]},
          "alerts": [
            {
              "status": "firing",
              "labels": {"alertname": "X"},
              "annotations": {},
              "aFieldFromAFutureRelease": true
            }
          ]
        }"#;

        let arrived = arrived(future.as_bytes()).expect("a future payload still parses");
        assert_eq!(arrived.notification.alerts().len(), 1);
        // And the unmodelled field is still in what a fallback would show.
        assert!(
            arrived.as_sent[0].contains("aFieldFromAFutureRelease"),
            "{}",
            arrived.as_sent[0]
        );
    }

    /// The as-sent text is what the fallback shows, so it has to carry the
    /// fields this service does not model -- which is the whole reason it is
    /// passed alongside the parsed Notification rather than derived from it.
    #[test]
    fn the_as_sent_text_keeps_what_the_domain_does_not_model() {
        let arrived = arrived(REAL.as_bytes()).expect("a real payload");

        assert_eq!(arrived.as_sent.len(), 2);
        for field in ["fingerprint", "startsAt", "generatorURL"] {
            assert!(
                arrived.as_sent[0].contains(field),
                "{field} was dropped: {}",
                arrived.as_sent[0]
            );
        }
        assert!(arrived.as_sent[0].contains("a1b2c3d4e5f6"));
        assert!(arrived.as_sent[1].contains("f6e5d4c3b2a1"));
    }

    /// A status that is not one of the two is a parse failure, not a third
    /// variant -- `pending` is a real Prometheus RULE state, so it is the
    /// mistake somebody would actually make.
    #[test]
    fn a_status_that_is_not_firing_or_resolved_is_refused() {
        for status in ["pending", "FIRING", "", "inactive"] {
            let body = format!(
                r#"{{"alerts":[{{"status":"{status}","labels":{{}},"annotations":{{}}}}]}}"#
            );
            assert_eq!(
                arrived(body.as_bytes()).err(),
                Some(NotAPayload::UnknownStatus),
                "status {status:?} was accepted"
            );
        }
    }

    /// Three shapes that are not a payload, each answered rather than panicked
    /// on. The empty-alerts case is the domain's own rule surfacing.
    #[test]
    fn a_body_that_is_not_a_payload_is_refused_by_name() {
        assert_eq!(
            arrived(b"not json at all").err(),
            Some(NotAPayload::NotAnObject)
        );
        assert_eq!(arrived(b"[1,2,3]").err(), Some(NotAPayload::NotAnObject));
        assert_eq!(arrived(b"{}").err(), Some(NotAPayload::NoAlertsArray));
        assert_eq!(
            arrived(br#"{"alerts":"not an array"}"#).err(),
            Some(NotAPayload::NoAlertsArray)
        );

        let empty = arrived(br#"{"alerts":[]}"#).expect_err("no alerts");
        assert!(matches!(empty, NotAPayload::Unusable(_)), "{empty:?}");
    }

    #[test]
    fn a_real_payload_yields_its_alerts_in_order() {
        let arrived = arrived(REAL.as_bytes()).expect("a real payload");

        let states: Vec<AlertState> = arrived
            .notification
            .alerts()
            .iter()
            .map(Alert::state)
            .collect();
        assert_eq!(states, vec![AlertState::Firing, AlertState::Resolved]);

        let first = &arrived.notification.alerts()[0];
        assert_eq!(first.labels().get("severity"), Some("critical"));
        assert_eq!(first.annotations().get("summary"), Some("disk is full"));
        // An annotation map that was empty stays empty rather than becoming a
        // present-but-blank value.
        assert_eq!(
            arrived.notification.alerts()[1]
                .annotations()
                .get("summary"),
            None
        );
    }
}
