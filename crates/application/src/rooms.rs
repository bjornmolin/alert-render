// SPDX-FileCopyrightText: 2026 Digg - Agency for Digital Government
// SPDX-License-Identifier: EUPL-1.2 OR GPL-3.0-or-later

//! Looking up where a Binding's Room actually is.

use async_trait::async_trait;
use domain::room::{MalformedRoom, Room};

/// Why the Room a Binding names could not be found.
///
/// The cases are separated by WHAT A HUMAN WOULD DO ABOUT THEM, which is the
/// only useful reason to split an error: a missing key is a Binding naming
/// something the deployment does not mount; an unreadable one is file
/// permissions; a malformed one is the wrong value in the right place.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoomUnavailable {
    /// Nothing is mounted under this key. The key is named; it is not secret.
    #[error("no room is mounted under the key `{key}`")]
    NoSuchKey {
        /// The key the Binding asked for.
        key: String,
    },
    /// Something is mounted under this key and could not be read.
    ///
    /// SEPARATE FROM `NoSuchKey` ON PURPOSE. A file that is present and
    /// unreadable reports as ABSENT in the obvious implementation, and upstream
    /// lost an hour to exactly that: a secret rendered with permissions other
    /// than 0440 looked like a secret that was never rendered at all.
    #[error("the room mounted under the key `{key}` could not be read")]
    Unreadable {
        /// The key the Binding asked for.
        key: String,
    },
    /// The value is there and is not a usable address.
    #[error("the room mounted under the key `{key}` is not a usable address: {why:?}")]
    Malformed {
        /// The key the Binding asked for.
        key: String,
        /// What is wrong with the value. Never contains the value.
        why: MalformedRoom,
    },
}

/// Where the Room named by a Binding's key can be reached.
///
/// Stated as a need and nothing more: a key goes in, a Room comes out. No path,
/// no file, no secret manager and no URL type appears here, so the deployment
/// is free to satisfy it with a Kubernetes Secret, an OpenBao-rendered tmpfs,
/// or anything else, without the application differing between environments.
#[async_trait]
pub trait RoomDirectory: Send + Sync {
    /// The Room mounted under `key`.
    ///
    /// Called ONCE PER DELIVERY rather than cached at startup. A Room's address
    /// is rotated on the chat instance by whoever administers it -- not by this
    /// service and not by the tenant -- so a value read once at startup means a
    /// third party can silently stop delivery until an operator notices and
    /// rolls a pod.
    async fn room(&self, key: &str) -> Result<Room, RoomUnavailable>;
}
