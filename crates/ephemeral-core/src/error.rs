//! The crate's error type.
//!
//! Errors here are meant to be shown to people. Ephemeral does things on a
//! user's behalf autonomously, so when something goes wrong the message has to
//! carry enough context for someone who did not read this code to understand
//! what happened — which application, which state, which permission.
//!
//! Every variant wraps a specific module error rather than flattening into
//! strings, so callers can match on the cause when they need to.

use crate::identity::{AppId, IdError};

/// The result type used throughout this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Something went wrong in the domain layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An identifier was not well formed.
    #[error("invalid identifier: {0}")]
    Id(#[from] IdError),

    /// A lifecycle transition was not permitted.
    #[error(transparent)]
    Lifecycle(#[from] crate::lifecycle::LifecycleError),

    /// The requested application does not exist.
    #[error("no application with id {id}")]
    AppNotFound {
        /// The id that was looked up.
        id: AppId,
    },

    /// An application with this id already exists.
    #[error("an application with id {id} already exists")]
    AppExists {
        /// The id that collided.
        id: AppId,
    },
}
