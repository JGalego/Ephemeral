//! The two permission systems.
//!
//! Ephemeral has two entirely separate permission spaces, and conflating them
//! would be a security failure rather than a refactor. They are separate types
//! that cannot be substituted for one another:
//!
//! | | Who holds it | What it governs |
//! |---|---|---|
//! | [`MetaPermission`] | Ephemeral itself | Running Docker, installing runtimes, executing processes, reaching the network, the keychain, the camera |
//! | [`AppPermission`] | One generated application | Exactly what that app may touch, scoped as narrowly as practical |
//!
//! ## The rules
//!
//! 1. **No inheritance, in either direction.** A grant names exactly one
//!    principal. Ephemeral holding `filesystem.read(~/**)` grants a generated
//!    app nothing at all.
//! 2. **Default deny.** Only an explicit, unexpired, unrevoked `Allow` naming
//!    that principal and covering that request permits an operation.
//! 3. **An explicit `Deny` wins**, whenever it was recorded.
//! 4. **Only a person decides.** [`PermissionLedger::decide`] refuses any actor
//!    but [`Actor::User`](crate::actor::Actor::User). The generation agent
//!    cannot grant a permission — to itself or to an app it wrote — whatever a
//!    model was persuaded to output. This is the structural defence against
//!    prompt injection.
//! 5. **Ephemeral's permission is a ceiling, not a source.** For an app to do
//!    something, both it *and* Ephemeral must be permitted, so revoking a
//!    meta-permission disables that capability product-wide. See
//!    [`PermissionLedger::check_app`].
//!
//! See [ADR-0003] for the alternatives that were rejected — a single permission
//! set, inheritance with narrowing, object capabilities, and deferring to the OS
//! sandbox alone.
//!
//! [ADR-0003]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0003-two-tier-permission-model.md
//!
//! ## Asking
//!
//! A permission question is a [`PermissionPrompt`], not a string. It carries
//! structured answers to what is asking, what it wants, why, what happens if you
//! allow it, and whether you can take it back — so no interface can ship a
//! meaningless *"Allow filesystem access?"* dialog.
//!
//! # Example
//!
//! ```
//! use ephemeral_core::{Actor, AppId, Principal};
//! use ephemeral_core::permission::{
//!     AppPermission, Decision, MetaPermission, PathScope, PermissionLedger,
//! };
//!
//! let downloads = PathScope::parse("~/Downloads/apartments/**")?;
//! let comparator = AppId::parse("apartment-comparator")?;
//!
//! let mut ledger = PermissionLedger::new();
//!
//! // Ephemeral may read the user's home directory.
//! ledger.allow(
//!     Principal::Ephemeral,
//!     MetaPermission::read(PathScope::parse("~/**")?),
//!     Actor::User,
//!     "so it can open the files you point it at",
//! )?;
//!
//! // That grants the generated app precisely nothing.
//! assert_eq!(
//!     ledger.check(
//!         &Principal::app(comparator.clone()),
//!         &AppPermission::read(downloads.clone()).into(),
//!     ),
//!     Decision::Deny,
//! );
//!
//! // The app has to be allowed separately, and only by a person.
//! ledger.allow(
//!     Principal::app(comparator.clone()),
//!     AppPermission::read(downloads.clone()),
//!     Actor::User,
//!     "to compare the CSV files you selected",
//! )?;
//!
//! assert!(ledger.check_app(&comparator, &AppPermission::read(downloads)).is_allowed());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod app;
mod grant;
mod ledger;
mod meta;
mod prompt;
mod scope;

pub use app::{
    AppPermission, AppPermissions, DevicePolicy, FilesystemRule, NetworkPolicy, ProcessPolicy,
};
pub use grant::{Decision, Grant, Permission};
pub use ledger::{EffectiveDecision, PermissionError, PermissionLedger};
pub use meta::MetaPermission;
pub use prompt::{PermissionPrompt, RiskLevel};
pub use scope::{HostScope, PathScope, ScopeError};
