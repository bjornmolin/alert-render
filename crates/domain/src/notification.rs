// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! What Alertmanager grouped and sent, and the Alerts inside it.

use std::collections::BTreeMap;

/// Whether an Alert is firing or has resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertState {
    /// The condition currently holds.
    Firing,
    /// The condition held and no longer does.
    Resolved,
}

/// Name/value pairs, shared by `Labels` and `Annotations`.
///
/// Private on purpose: the two are kept distinct in every public signature.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Pairs(BTreeMap<String, String>);

impl Pairs {
    /// The value for `name`, or `None` if it was never set -- or was set to
    /// nothing, which Prometheus treats as the same thing.
    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
}

impl<const N: usize> From<[(&str, &str); N]> for Pairs {
    fn from(pairs: [(&str, &str); N]) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        )
    }
}

/// What identifies an Alert: `alertname`, `severity`, `namespace` and so on.
///
/// A SEPARATE TYPE FROM `Annotations`, although the shape is identical. They
/// are two arguments of the same shape to the same constructor, which is one
/// transposition away from a silent bug -- labels rendered as descriptions and
/// descriptions matched on as identity. Distinct newtypes make that a compile
/// error instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Labels(Pairs);

impl Labels {
    /// The value for `name`, or `None` if it was never set or was set to the
    /// empty string -- which Prometheus treats as equivalent to absent.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name)
    }
}

impl<const N: usize> From<[(&str, &str); N]> for Labels {
    fn from(pairs: [(&str, &str); N]) -> Self {
        Self(Pairs::from(pairs))
    }
}

/// What describes an Alert to a human: `summary`, `description`, `runbook_url`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Annotations(Pairs);

impl Annotations {
    /// The value for `name`, or `None` if it was never set or was set to the
    /// empty string.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name)
    }
}

impl<const N: usize> From<[(&str, &str); N]> for Annotations {
    fn from(pairs: [(&str, &str); N]) -> Self {
        Self(Pairs::from(pairs))
    }
}

/// A `status` this service does not recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownAlertState;

impl core::str::FromStr for AlertState {
    type Err = UnknownAlertState;

    fn from_str(spelling: &str) -> Result<Self, Self::Err> {
        // EXACT MATCH, not case-insensitive and not trimmed. Alertmanager
        // sends these two spellings and nothing else; accepting "Firing" or
        // " firing" would be inventing tolerance for a sender that does not
        // need it, and the tolerance is where a typo stops being visible.
        match spelling {
            "firing" => Ok(Self::Firing),
            "resolved" => Ok(Self::Resolved),
            _ => Err(UnknownAlertState),
        }
    }
}

/// One condition that is firing or has resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    state: AlertState,
    labels: Labels,
    annotations: Annotations,
}

impl Alert {
    /// An Alert in the given state, with what identifies and describes it.
    #[must_use]
    pub fn new(state: AlertState, labels: Labels, annotations: Annotations) -> Self {
        Self {
            state,
            labels,
            annotations,
        }
    }

    /// Whether this Alert is firing or has resolved.
    #[must_use]
    pub fn state(&self) -> AlertState {
        self.state
    }

    /// What identifies this Alert.
    #[must_use]
    pub fn labels(&self) -> &Labels {
        &self.labels
    }

    /// What describes this Alert to a human.
    #[must_use]
    pub fn annotations(&self) -> &Annotations {
        &self.annotations
    }
}

/// Why a Notification could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedNotification {
    /// It carried no Alerts at all.
    NoAlerts,
}

/// A group of Alerts that Alertmanager's routing decided belong together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    alerts: Vec<Alert>,
}

impl Notification {
    /// A Notification carrying these Alerts, or why it could not be one.
    ///
    /// # Errors
    ///
    /// [`MalformedNotification::NoAlerts`] if `alerts` is empty. Alertmanager
    /// does not send an empty group, so refusing it here is what keeps every
    /// consumer downstream free of a branch for a situation that cannot arise.
    pub fn new(alerts: Vec<Alert>) -> Result<Self, MalformedNotification> {
        if alerts.is_empty() {
            return Err(MalformedNotification::NoAlerts);
        }
        Ok(Self { alerts })
    }

    /// The Alerts, in the order they arrived.
    #[must_use]
    pub fn alerts(&self) -> &[Alert] {
        &self.alerts
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Alert, AlertState, Annotations, Labels, MalformedNotification, Notification,
        UnknownAlertState,
    };

    /// PROMETHEUS'S OWN RULE, not an invention of ours: a label with an empty
    /// value is equivalent to the label not being there. In `PromQL`, a matcher
    /// of `{foo=""}` matches series that have no `foo` at all, so the two are the same thing
    /// upstream and must be the same thing here.
    ///
    /// Without this, a template guarding on `severity` renders "severity: " for
    /// an alert whose severity was set to nothing, and the reader cannot tell
    /// that from a real value.
    #[test]
    fn a_label_set_to_nothing_is_the_same_as_absent() {
        let alert = Alert::new(
            AlertState::Firing,
            Labels::from([("severity", ""), ("team", "wallet")]),
            Annotations::from([("summary", "")]),
        );

        assert_eq!(alert.labels().get("severity"), None);
        assert_eq!(alert.labels().get("team"), Some("wallet"));
        assert_eq!(alert.annotations().get("summary"), None);
    }

    /// An absent label is absent. `Option` means genuinely optional here, and
    /// a template asking for a label nobody set must be able to tell that from
    /// a label set to nothing -- otherwise it renders "severity: " and the
    /// reader cannot tell which happened.
    #[test]
    fn a_label_or_annotation_that_was_never_set_is_absent() {
        let alert = Alert::new(
            AlertState::Firing,
            Labels::from([("severity", "critical")]),
            Annotations::from([("summary", "disk is full")]),
        );

        assert_eq!(alert.labels().get("severity"), Some("critical"));
        assert_eq!(alert.labels().get("team"), None);
        assert_eq!(alert.annotations().get("summary"), Some("disk is full"));
        assert_eq!(alert.annotations().get("runbook_url"), None);
    }

    /// The wire carries `status` as a string. It must not survive as one:
    /// `"firing"` and `"resolved"` are the only two things Alertmanager sends,
    /// so a third is a parse failure rather than a variant, and no code
    /// downstream should ever compare a status by spelling.
    #[test]
    fn a_state_is_parsed_from_the_wire_spelling_or_refused() {
        assert_eq!("firing".parse(), Ok(AlertState::Firing));
        assert_eq!("resolved".parse(), Ok(AlertState::Resolved));
        assert_eq!("pending".parse::<AlertState>(), Err(UnknownAlertState));
        assert_eq!("".parse::<AlertState>(), Err(UnknownAlertState));
    }

    /// PARSE, DON'T VALIDATE. Alertmanager does not send an empty group, so
    /// accepting one would mean every consumer downstream carrying a branch for
    /// a situation that cannot arise -- and one of them eventually rendering an
    /// empty message into a room. The constructor is where that is refused,
    /// once.
    #[test]
    fn a_notification_with_no_alerts_is_refused_by_name() {
        let refused = Notification::new(Vec::new());
        assert_eq!(refused, Err(MalformedNotification::NoAlerts));
    }

    /// THE TRAP THIS BEAD NAMES. Alertmanager almost always groups several
    /// Alerts into one Notification, so a renderer written against "the alert"
    /// renders the first and silently drops the rest. Count and order are
    /// asserted together because a type that loses one usually loses both.
    #[test]
    fn a_notification_keeps_every_alert_in_the_order_it_arrived() {
        let notification = Notification::new(vec![
            Alert::new(
                AlertState::Firing,
                Labels::default(),
                Annotations::default(),
            ),
            Alert::new(
                AlertState::Resolved,
                Labels::default(),
                Annotations::default(),
            ),
            Alert::new(
                AlertState::Firing,
                Labels::default(),
                Annotations::default(),
            ),
        ])
        .expect("three alerts is a well-formed notification");

        let states: Vec<AlertState> = notification.alerts().iter().map(Alert::state).collect();
        assert_eq!(
            states,
            vec![AlertState::Firing, AlertState::Resolved, AlertState::Firing]
        );
    }
}
