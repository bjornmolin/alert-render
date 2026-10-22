// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! The Bindings set, read from a mounted YAML file.
//!
//! NO PORT, and that is deliberate. Nothing in the application ASKS for
//! configuration at runtime, so there is nothing to state as a need: the
//! composition root reads this file once and hands the Bindings to the things
//! that use them. A trait here would have one implementation and one caller,
//! invented to satisfy a pattern.
//!
//! READ ONCE AT STARTUP, unlike a Room address, which `mounted-rooms` re-reads
//! per delivery. The asymmetry is the point. Bindings change through gitops, so
//! a change should roll the pods -- and the way to get that is a `ConfigMap` whose
//! NAME carries a hash of its contents, which beats watching a file because
//! there is nothing to remember to update. A Room address rotates out of band
//! on the chat instance, by somebody who is neither this service nor the
//! tenant, so that one cannot wait for a roll.

use std::path::{Path, PathBuf};

use domain::binding::{Binding, Bindings, MalformedBinding, MalformedBindings};

/// Why the Bindings could not be read.
#[derive(Debug, thiserror::Error)]
pub enum BindingsUnreadable {
    /// Nothing is mounted at that path.
    #[error("no bindings file at {path}")]
    NoSuchFile {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// Something is mounted and could not be read.
    #[error("the bindings file at {path} could not be read")]
    Unreadable {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// It is not the YAML this expects.
    #[error("the bindings file at {path} is not readable as bindings: {why}")]
    Unparseable {
        /// Where it was looked for.
        path: PathBuf,
        /// What the reader objected to.
        why: String,
    },
    /// A single binding is not usable.
    #[error("a binding in {path} is not usable: {why:?}")]
    BadBinding {
        /// Where it was looked for.
        path: PathBuf,
        /// What the domain refused.
        why: MalformedBinding,
    },
    /// The set as a whole is not usable.
    #[error("the bindings in {path} are not a usable set: {why:?}")]
    BadSet {
        /// Where it was looked for.
        path: PathBuf,
        /// What the domain refused.
        why: MalformedBindings,
    },
}

// THESE TYPES EXIST ONLY TO BE DESERIALISED, which is why they are here and not
// in the domain: this crate owns the file format, and the domain gains no serde.
//
// `deny_unknown_fields` on both, and it is the rule people argue about. A typo
// in `tempalte:` would otherwise produce a binding with no template and an
// error at the first delivery -- a long way from the cause, in a tenant's room
// rather than in a startup log.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    bindings: Vec<Row>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    listener: String,
    template: String,
    room: String,
}

/// The Bindings declared in the file at `path`.
///
/// # Errors
///
/// [`BindingsUnreadable`], distinguishing a file that is absent from one that
/// cannot be read, one that is not the expected YAML, and one whose contents
/// the domain refuses. Those are four different things to do something about,
/// and collapsing them into "invalid configuration" moves the work of telling
/// them apart onto whoever is reading the log at the time.
pub fn bindings_at(path: impl AsRef<Path>) -> Result<Bindings, BindingsUnreadable> {
    let path = path.as_ref();

    let text = std::fs::read_to_string(path).map_err(|why| match why.kind() {
        std::io::ErrorKind::NotFound => BindingsUnreadable::NoSuchFile {
            path: path.to_path_buf(),
        },
        _ => BindingsUnreadable::Unreadable {
            path: path.to_path_buf(),
        },
    })?;

    let file: File =
        serde_yaml_ng::from_str(&text).map_err(|why| BindingsUnreadable::Unparseable {
            path: path.to_path_buf(),
            why: why.to_string(),
        })?;

    // The domain owns every rule about a binding and about the set. This maps
    // and surfaces; it does not re-validate, and it does not translate a
    // domain refusal into a message of its own -- the reason the domain gives
    // names the path or the field, and that is the useful half.
    let rows = file
        .bindings
        .into_iter()
        .map(|row| {
            Binding::new(&row.listener, &row.template, &row.room).map_err(|why| {
                BindingsUnreadable::BadBinding {
                    path: path.to_path_buf(),
                    why,
                }
            })
        })
        .collect::<Result<Vec<Binding>, _>>()?;

    Bindings::new(rows).map_err(|why| BindingsUnreadable::BadSet {
        path: path.to_path_buf(),
        why,
    })
}

#[cfg(test)]
mod tests {
    use super::{BindingsUnreadable, bindings_at};
    use domain::binding::{Binding, MalformedBinding, MalformedBindings};

    fn written(yaml: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("bindings.yaml");
        std::fs::write(&path, yaml).expect("write it");
        (dir, path)
    }

    /// THE DOMAIN'S REASON MUST SURVIVE THE MAPPING, which is what this
    /// asserts rather than "a duplicate is refused" -- the domain already
    /// refuses it, and the thing that can go wrong here is flattening its
    /// answer into "invalid configuration" and losing the path.
    #[test]
    fn a_duplicate_listener_is_refused_and_the_path_survives() {
        let (_dir, path) = written(
            r"
bindings:
  - listener: /notify/team
    template: team
    room: team-room-url
  - listener: /notify/team
    template: other
    room: other-room-url
",
        );

        let refused = bindings_at(&path).expect_err("a duplicate listener");

        assert!(
            matches!(
                refused,
                BindingsUnreadable::BadSet {
                    why: MalformedBindings::DuplicateListener(ref p),
                    ..
                } if p == "/notify/team"
            ),
            "{refused:?}"
        );
        assert!(refused.to_string().contains("/notify/team"), "{refused}");
    }

    /// Same again for a single binding's rules: the reserved path is the
    /// domain's rule, and the error has to still say which rule.
    #[test]
    fn a_binding_on_health_is_refused_by_name() {
        let (_dir, path) = written(
            r"
bindings:
  - listener: /health
    template: t
    room: k
",
        );

        let refused = bindings_at(&path).expect_err("/health is reserved");

        assert!(
            matches!(
                refused,
                BindingsUnreadable::BadBinding {
                    why: MalformedBinding::ReservedPath,
                    ..
                }
            ),
            "{refused:?}"
        );
    }

    /// FOUR DIFFERENT THINGS TO DO SOMETHING ABOUT, so four errors. Absent is a
    /// mount that did not happen; unparseable is a file somebody edited;
    /// an empty set is a file that parsed and says nothing. Collapsing them
    /// moves the work of telling them apart onto whoever reads the log.
    #[test]
    fn absent_unparseable_and_empty_are_distinct() {
        let (dir, _path) = written(
            "bindings: []
",
        );

        let absent = bindings_at(dir.path().join("nope.yaml")).expect_err("absent");
        assert!(
            matches!(absent, BindingsUnreadable::NoSuchFile { .. }),
            "{absent:?}"
        );

        let (_d2, bad) = written(
            "bindings: [ this is not: valid: yaml
",
        );
        let unparseable = bindings_at(&bad).expect_err("unparseable");
        assert!(
            matches!(unparseable, BindingsUnreadable::Unparseable { .. }),
            "{unparseable:?}"
        );

        let (_d3, empty) = written(
            "bindings: []
",
        );
        let none = bindings_at(&empty).expect_err("no bindings");
        assert!(
            matches!(
                none,
                BindingsUnreadable::BadSet {
                    why: MalformedBindings::NoBindings,
                    ..
                }
            ),
            "{none:?}"
        );
    }

    /// AN UNKNOWN KEY IS REFUSED, NOT IGNORED. `tempalte:` would otherwise
    /// deserialise into a binding with an empty template and fail at the first
    /// delivery -- far from the cause, and in a tenant's room rather than in a
    /// startup log.
    #[test]
    fn a_misspelled_key_is_refused_rather_than_ignored() {
        let (_dir, path) = written(
            r"
bindings:
  - listener: /notify/team
    tempalte: team
    room: team-room-url
",
        );

        let refused = bindings_at(&path).expect_err("a misspelled key");

        assert!(
            matches!(refused, BindingsUnreadable::Unparseable { .. }),
            "{refused:?}"
        );
        assert!(refused.to_string().contains("tempalte"), "{refused}");
    }

    /// THE LEAK THIS FILE FORMAT IS SHAPED TO PREVENT, asserted here because
    /// this is where a human writes the mistake. The `room` field names a
    /// secret key; pasting the ADDRESS in puts a credential into a `ConfigMap`
    /// in a gitops repository, where it is published into git history and into
    /// `kubectl get cm -o yaml`.
    ///
    /// The domain refuses it (a rule added while writing this test), and this
    /// asserts the refusal survives the mapping -- and that the error does not
    /// then quote the address it was refusing.
    #[test]
    fn an_address_pasted_into_the_room_field_is_refused_without_quoting_it() {
        let (_dir, path) = written(
            r"
bindings:
  - listener: /notify/team
    template: team
    room: https://hookshot.example.invalid/0191a7c2-SECRET
",
        );

        let refused = bindings_at(&path).expect_err("an address is not a key");

        assert!(
            matches!(
                refused,
                BindingsUnreadable::BadBinding {
                    why: MalformedBinding::RoomKeyLooksLikeAnAddress,
                    ..
                }
            ),
            "{refused:?}"
        );
        // The whole point is that the value is secret, so the complaint about
        // it must not repeat it.
        let shown = format!("{refused} / {refused:?}");
        assert!(!shown.contains("SECRET"), "the address leaked: {shown}");
        assert!(!shown.contains("hookshot.example.invalid"), "{shown}");
    }

    /// Order is asserted, not just membership: the file is the only place that
    /// decides it, and a reader that returned a set would lose it silently.
    #[test]
    fn a_file_with_two_bindings_yields_both_in_order() {
        let (_dir, path) = written(
            r"
bindings:
  - listener: /notify/critical
    template: urgent
    room: oncall-room-url
  - listener: /notify/warning
    template: quiet
    room: team-room-url
",
        );

        let bindings = bindings_at(&path).expect("a well-formed file");

        let listeners: Vec<&str> = ["/notify/critical", "/notify/warning"]
            .into_iter()
            .filter(|p| bindings.for_listener(p).is_some())
            .collect();
        assert_eq!(listeners, vec!["/notify/critical", "/notify/warning"]);
        assert_eq!(
            bindings
                .for_listener("/notify/critical")
                .map(Binding::template),
            Some("urgent")
        );
        assert_eq!(
            bindings
                .for_listener("/notify/warning")
                .map(Binding::room_key),
            Some("team-room-url")
        );
    }
}
