// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Room addresses read from a mounted directory, one file per key.
//!
//! WHO WRITES THE FILES IS NOT THIS ADAPTER'S CONCERN, and that is the point. A
//! Kubernetes Secret and an `OpenBao` agent rendering into a tmpfs look identical
//! from here, so the dev cluster can use the simpler one without the
//! application differing from production. That matters concretely: dev-mode
//! `OpenBao` is in-memory and loses its contents on every restart, and
//! webhook-proxy's committed vault-agent configuration has never authenticated
//! at all, because its Kubernetes auth role is the platform's to create.
//!
//! A DIRECTORY OF FILES AND NOT ENVIRONMENT VARIABLES. An environment variable
//! is readable from `/proc/<pid>/environ`, is inherited by every child process,
//! shows up in `kubectl exec -- env`, and is the value most likely to be
//! captured by a crash handler or a tracing agent. A room address is the whole
//! credential, so it gets the narrower channel.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use application::rooms::{RoomDirectory, RoomUnavailable};
use async_trait::async_trait;
use domain::room::Room;

/// Reads Room addresses from one directory, one file per key.
pub struct MountedRooms {
    directory: PathBuf,
}

impl MountedRooms {
    /// Rooms mounted under `directory`.
    pub fn at(directory: impl AsRef<Path>) -> Self {
        Self {
            directory: directory.as_ref().to_path_buf(),
        }
    }
}

#[async_trait]
impl RoomDirectory for MountedRooms {
    async fn room(&self, key: &str) -> Result<Room, RoomUnavailable> {
        let contents = std::fs::read_to_string(self.directory.join(key));
        let raw = contents.map_err(|why| match why.kind() {
            ErrorKind::NotFound => RoomUnavailable::NoSuchKey {
                key: key.to_owned(),
            },
            _ => RoomUnavailable::Unreadable {
                key: key.to_owned(),
            },
        })?;

        Room::new(&raw).map_err(|why| RoomUnavailable::Malformed {
            key: key.to_owned(),
            why,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::MountedRooms;
    use application::rooms::{RoomDirectory, RoomUnavailable};
    use std::fs;

    const ADDRESS: &str = "https://hookshot.example.invalid/0191a7c2-SECRET-UUID";

    fn mounted(files: &[(&str, &str)]) -> (tempfile::TempDir, MountedRooms) {
        let dir = tempfile::tempdir().expect("a temp dir");
        for (key, value) in files {
            fs::write(dir.path().join(key), value).expect("write the key");
        }
        let rooms = MountedRooms::at(dir.path());
        (dir, rooms)
    }

    /// ABSENT AND UNREADABLE ARE DIFFERENT, and upstream lost an hour to the
    /// implementation that conflates them: a secret rendered with permissions
    /// other than 0440 reports as a secret that was never rendered at all. The
    /// fixes are nothing alike -- one is a Binding naming a key the deployment
    /// does not mount, the other is file permissions -- so the error has to say
    /// which.
    #[tokio::test]
    async fn an_absent_key_is_distinct_from_one_that_cannot_be_read() {
        let (dir, rooms) = mounted(&[("team-room-url", ADDRESS)]);

        assert_eq!(
            rooms.room("no-such-key").await,
            Err(RoomUnavailable::NoSuchKey {
                key: "no-such-key".to_owned()
            })
        );

        // A directory where a file is expected is readable-but-not-a-file,
        // which is the portable way to provoke the second case: chmod 0 is a
        // no-op for root, and tests run as root in a container.
        fs::create_dir(dir.path().join("a-directory")).expect("mkdir");
        assert_eq!(
            rooms.room("a-directory").await,
            Err(RoomUnavailable::Unreadable {
                key: "a-directory".to_owned()
            })
        );
    }

    /// A key whose file holds the wrong VALUE is a third case again: the mount
    /// worked and the content is wrong. The usual wrong content is the key name
    /// itself, copied from the Binding.
    #[tokio::test]
    async fn a_key_holding_something_that_is_not_an_address_says_so() {
        let (_dir, rooms) = mounted(&[("team-room-url", "team-room-url")]);

        let refused = rooms
            .room("team-room-url")
            .await
            .expect_err("not an address");

        assert!(
            matches!(refused, RoomUnavailable::Malformed { .. }),
            "{refused:?}"
        );
    }

    /// RE-READ PER CALL, which is the decision this bead departs from
    /// webhook-proxy on. It reads its secrets once into a map at startup and
    /// relies on a pod roll; right for secrets it mints itself. A Room address
    /// is rotated on the chat instance by someone else, so a read-once design
    /// lets a third party silently stop delivery until an operator notices.
    #[tokio::test]
    async fn a_rotated_address_is_picked_up_without_a_restart() {
        let (dir, rooms) = mounted(&[("team-room-url", ADDRESS)]);
        assert_eq!(
            rooms.room("team-room-url").await.expect("first").exposed(),
            ADDRESS
        );

        let rotated = "https://hookshot.example.invalid/0191b000-ROTATED";
        fs::write(dir.path().join("team-room-url"), rotated).expect("rotate it");

        assert_eq!(
            rooms.room("team-room-url").await.expect("second").exposed(),
            rotated,
            "the same instance kept serving the old address"
        );
    }

    /// TWO BINDINGS MAY NAME ONE KEY -- two severities with two Templates into
    /// one room is legitimate, and only a Binding's LISTENER has to be unique.
    /// So the same key resolving twice is not a case to guard against.
    #[tokio::test]
    async fn one_key_may_be_asked_for_more_than_once() {
        let (_dir, rooms) = mounted(&[("team-room-url", ADDRESS)]);

        let first = rooms.room("team-room-url").await.expect("first");
        let second = rooms.room("team-room-url").await.expect("second");

        assert_eq!(first, second);
    }

    /// Neither the error type nor the Room may carry the address into a log.
    /// Asserted on formatted output, including the `Display` that `thiserror`
    /// generates -- which is the one most likely to end up in a log line.
    #[tokio::test]
    async fn no_formatting_of_an_error_or_a_room_reveals_an_address() {
        let (_dir, rooms) = mounted(&[("bad", "not-an-address-but-secret-ish")]);

        let refused = rooms.room("bad").await.expect_err("not an address");
        let shown = format!("{refused} / {refused:?}");
        assert!(!shown.contains("secret-ish"), "the value leaked: {shown}");
        assert!(shown.contains("bad"), "the key should be named: {shown}");
    }

    #[tokio::test]
    async fn a_mounted_key_yields_its_room() {
        let (_dir, rooms) = mounted(&[("team-room-url", ADDRESS)]);

        let room = rooms
            .room("team-room-url")
            .await
            .expect("the key is mounted");

        assert_eq!(room.exposed(), ADDRESS);
    }
}
