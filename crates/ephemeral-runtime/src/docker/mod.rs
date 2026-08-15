//! The Docker-backed runtime.
//!
//! Split in two along the line that matters. [`command`] decides the
//! confinement — it turns a [`ContainerSpec`](crate::spec::ContainerSpec) into
//! an argument vector, and it is pure, so every hardening flag is a unit test
//! that runs where no container daemon exists. [`DockerRuntime`] does nothing
//! but spawn what `command` decided, and is the only part of this crate that
//! touches a process ([ADR-0014]).
//!
//! The split is enforced rather than intended: building this crate without the
//! `daemon` feature removes the spawning half and keeps the deciding half, and
//! CI builds it that way.
//!
//! Podman works too, through its `docker` shim — the argument vectors here use
//! nothing Podman does not implement.
//!
//! [ADR-0014]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0014-drive-docker-through-its-cli.md

pub mod command;

#[cfg(feature = "daemon")]
mod runtime;

#[cfg(feature = "daemon")]
pub use runtime::{COMMAND_VARIABLE, DockerRuntime};
