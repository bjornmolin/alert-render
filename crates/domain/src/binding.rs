// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! What a tenant declares: which path, rendered by which Template, into which Room.

/// Why a Binding could not be constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MalformedBinding {
    /// The listener path did not begin with `/`.
    PathNotAbsolute,
    /// The listener path is one this service serves itself.
    ReservedPath,
    /// The Template id was empty or only whitespace.
    EmptyTemplateId,
    /// The Room key was empty or only whitespace.
    EmptyRoomKey,
}

/// Paths this service answers itself, which a Binding may not claim.
///
/// `/metrics` is listed even though it will be served on a second listener, so
/// the refusal survives a later change to that decision. Costs nothing to keep.
const RESERVED_PATHS: &[&str] = &["/health", "/metrics"];

/// One listener path, the Template that renders it, and the Room it reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    listener: String,
    template: String,
    room: String,
}

impl Binding {
    /// A Binding, or why it could not be one.
    ///
    /// # Errors
    ///
    /// [`MalformedBinding::PathNotAbsolute`] if `listener` does not begin with
    /// `/`, and [`MalformedBinding::ReservedPath`] if it names one of this
    /// service's own paths. Both are refused rather than corrected: the tenant
    /// writes this path twice -- here and as a webhook URL in its
    /// `AlertmanagerConfig` -- and a silent correction would make one of the two
    /// wrong while the other worked.
    ///
    /// [`MalformedBinding::EmptyTemplateId`] and
    /// [`MalformedBinding::EmptyRoomKey`] for a missing or whitespace-only
    /// Template or Room key -- a path that would accept a Notification and have
    /// nowhere to put it.
    pub fn new(listener: &str, template: &str, room: &str) -> Result<Self, MalformedBinding> {
        if !listener.starts_with('/') {
            return Err(MalformedBinding::PathNotAbsolute);
        }
        if RESERVED_PATHS.contains(&listener) {
            return Err(MalformedBinding::ReservedPath);
        }
        if template.trim().is_empty() {
            return Err(MalformedBinding::EmptyTemplateId);
        }
        if room.trim().is_empty() {
            return Err(MalformedBinding::EmptyRoomKey);
        }
        Ok(Self {
            listener: listener.to_owned(),
            template: template.to_owned(),
            room: room.to_owned(),
        })
    }

    /// The path a Notification for this Binding arrives on.
    #[must_use]
    pub fn listener(&self) -> &str {
        &self.listener
    }

    /// Which Template renders it.
    #[must_use]
    pub fn template(&self) -> &str {
        &self.template
    }

    /// The SECRET KEY naming where this Binding's Room URI is read from.
    ///
    /// A key, never the URI. A Room URI is the whole credential and this
    /// configuration is a `ConfigMap` in a gitops repository; the URI would be
    /// published into git and into `kubectl get cm -o yaml`. The value is read
    /// from the mounted secret directory instead.
    #[must_use]
    pub fn room_key(&self) -> &str {
        &self.room
    }
}

/// Why a set of Bindings could not be constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MalformedBindings {
    /// There were none at all.
    NoBindings,
    /// Two Bindings claimed the same listener path, which is named.
    DuplicateListener(String),
}

/// Every Binding this deployment serves.
///
/// A TYPE RATHER THAN A `Vec`, because the rule that matters is about the
/// collection and not the element: two Bindings on one path is ambiguous, and
/// whichever won would be an implementation detail nobody should have to know.
/// Refused at construction, so no request-time lookup can be ambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings(Vec<Binding>);

impl Bindings {
    /// Every Binding, or why they are not a usable set.
    ///
    /// # Errors
    ///
    /// [`MalformedBindings::NoBindings`] for an empty set -- a deployment that
    /// would accept nothing and deliver nowhere is a configuration mistake, not
    /// a quiet idle state.
    ///
    /// [`MalformedBindings::DuplicateListener`], naming the path, if two
    /// Bindings claim it. The path is named because "a duplicate exists" sends
    /// a reader through the whole file.
    ///
    /// Note that two Bindings naming the same ROOM key is fine and not checked:
    /// two severities with two Templates into one room is a thing a tenant may
    /// legitimately want.
    pub fn new(bindings: Vec<Binding>) -> Result<Self, MalformedBindings> {
        if bindings.is_empty() {
            return Err(MalformedBindings::NoBindings);
        }
        let mut seen: Vec<&str> = Vec::with_capacity(bindings.len());
        for binding in &bindings {
            if seen.contains(&binding.listener()) {
                return Err(MalformedBindings::DuplicateListener(
                    binding.listener().to_owned(),
                ));
            }
            seen.push(binding.listener());
        }
        Ok(Self(bindings))
    }

    /// The Binding for `path`, matched EXACTLY.
    ///
    /// Not by prefix, and not ignoring a trailing slash. `/notify/teamx` is a
    /// different binding from `/notify/team` and must not reach it; a prefix
    /// match would additionally let a binding on `/` swallow everything,
    /// `/health` included. A path with no Binding is `None`, which the inbound
    /// adapter answers as a 404 rather than guessing.
    #[must_use]
    pub fn for_listener(&self, path: &str) -> Option<&Binding> {
        self.0.iter().find(|binding| binding.listener() == path)
    }
}

#[cfg(test)]
mod tests {
    use super::{Binding, Bindings, MalformedBinding, MalformedBindings};

    /// THE CASE THAT MUST NOT BE MISTAKEN FOR THE DUPLICATE. Two severities,
    /// two Templates, one room is something a tenant may legitimately want --
    /// "critical" phrased urgently and "warning" phrased quietly, both landing
    /// where the team is looking. Only the LISTENER has to be unique.
    #[test]
    fn two_bindings_may_share_one_room() {
        let shared = vec![
            Binding::new("/notify/critical", "urgent", "team-url").expect("well-formed"),
            Binding::new("/notify/warning", "quiet", "team-url").expect("well-formed"),
        ];
        let bindings = Bindings::new(shared).expect("sharing a room is legitimate");
        assert_eq!(
            bindings
                .for_listener("/notify/critical")
                .map(Binding::template),
            Some("urgent")
        );
        assert_eq!(
            bindings
                .for_listener("/notify/warning")
                .map(Binding::template),
            Some("quiet")
        );
    }

    /// A deployment with no Bindings would accept nothing and deliver nowhere,
    /// so it is a configuration mistake rather than a quiet idle state. Refused
    /// at startup, where somebody is looking.
    #[test]
    fn a_set_with_no_bindings_is_refused_by_name() {
        assert_eq!(
            Bindings::new(Vec::new()),
            Err(MalformedBindings::NoBindings)
        );
    }

    /// EXACT, NOT PREFIX. `/notify/teamx` is a different binding from
    /// `/notify/team` and must not reach it -- a prefix match would also make a
    /// binding on `/` swallow everything, `/health` included.
    #[test]
    fn a_listener_is_matched_exactly_and_never_by_prefix() {
        let bindings = Bindings::new(vec![
            Binding::new("/notify/team", "team", "team-url").expect("well-formed"),
        ])
        .expect("one binding is a set");

        assert!(bindings.for_listener("/notify/team").is_some());
        assert!(bindings.for_listener("/notify/teamx").is_none());
        assert!(bindings.for_listener("/notify").is_none());
        assert!(bindings.for_listener("/notify/team/").is_none());
        assert!(bindings.for_listener("/").is_none());
    }

    /// THE RULE THE SET EXISTS FOR. Two Bindings on one path would mean alerts
    /// arriving in the wrong room with the right formatting, which is a symptom
    /// that points at the template rather than at the configuration. The error
    /// names the path, because "a duplicate exists" sends a reader through the
    /// whole file.
    #[test]
    fn two_bindings_on_the_same_listener_are_refused_and_the_path_is_named() {
        let clash = vec![
            Binding::new("/notify/team", "team", "team-url").expect("well-formed"),
            Binding::new("/notify/oncall", "oncall", "oncall-url").expect("well-formed"),
            Binding::new("/notify/team", "other", "other-url").expect("well-formed"),
        ];
        assert_eq!(
            Bindings::new(clash),
            Err(MalformedBindings::DuplicateListener(
                "/notify/team".to_owned()
            ))
        );
    }

    /// A Binding with no Template or no Room key is not a partial
    /// configuration to be filled in later -- it is a path that would accept a
    /// Notification and have nowhere to put it. Whitespace counts, for the same
    /// reason it does in a Room URI: a key of one space is a key that will
    /// never be found, and the error should say so now rather than per request.
    #[test]
    fn a_binding_needs_a_template_and_a_room_key() {
        assert_eq!(
            Binding::new("/notify/x", "", "k"),
            Err(MalformedBinding::EmptyTemplateId)
        );
        assert_eq!(
            Binding::new("/notify/x", "  ", "k"),
            Err(MalformedBinding::EmptyTemplateId)
        );
        assert_eq!(
            Binding::new("/notify/x", "t", ""),
            Err(MalformedBinding::EmptyRoomKey)
        );
        assert_eq!(
            Binding::new("/notify/x", "t", "\n"),
            Err(MalformedBinding::EmptyRoomKey)
        );
    }

    /// A relative path is refused rather than helpfully prefixed. The tenant
    /// writes this path twice -- here and as the webhook URL in its
    /// `AlertmanagerConfig` -- and the two have to be the same string. Quietly
    /// turning `notify/x` into `/notify/x` would make one of the two wrong and
    /// the other work.
    #[test]
    fn a_listener_path_must_be_absolute() {
        assert_eq!(
            Binding::new("notify/oncall", "t", "k"),
            Err(MalformedBinding::PathNotAbsolute)
        );
        assert_eq!(
            Binding::new("", "t", "k"),
            Err(MalformedBinding::PathNotAbsolute)
        );
    }

    /// THE PATHS THIS SERVICE SERVES ITSELF cannot be bound. A binding on
    /// `/health` shadows the readiness probe and the symptom is a pod that
    /// never becomes ready for a reason that points at Kubernetes; one on
    /// `/metrics` hides the delivery counters, so a room going quiet becomes
    /// invisible at exactly the wrong moment.
    #[test]
    fn a_listener_path_may_not_claim_this_services_own_paths() {
        for reserved in ["/health", "/metrics"] {
            assert_eq!(
                Binding::new(reserved, "t", "k"),
                Err(MalformedBinding::ReservedPath),
                "{reserved} was accepted"
            );
        }
    }

    /// THE THREE THINGS A TENANT DECLARES, readable back as they were given.
    /// `room_key` is named for what it is so that nobody passes a URI to it:
    /// the type cannot stop them, but the name and its doc comment can.
    #[test]
    fn a_binding_carries_a_listener_a_template_and_a_room_key() {
        let binding = Binding::new("/notify/oncall", "oncall", "oncall-room-url")
            .expect("a well-formed binding");

        assert_eq!(binding.listener(), "/notify/oncall");
        assert_eq!(binding.template(), "oncall");
        assert_eq!(binding.room_key(), "oncall-room-url");
    }
}
