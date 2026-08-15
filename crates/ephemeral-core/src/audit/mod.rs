//! The audit log: an append-only, hash-chained record of security decisions.
//!
//! Two different people need answers from this record. A user asks *"what did
//! this thing do with my files?"* An incident responder asks *"was this
//! permission ever granted, by whom, and what ran afterwards?"* Both are
//! worthless if the record can be quietly edited, or so noisy that the important
//! entries are invisible.
//!
//! ## What makes this different from logging
//!
//! - **Append-only.** No update, no delete. Retention may age out old entries
//!   wholesale; an individual entry is never rewritten.
//! - **Hash-chained.** Each entry carries the hash of its predecessor and a hash
//!   of itself, so modification, reordering or excision within the chain is
//!   detectable by [`AuditLog::verify`].
//! - **Separate from observability.** Logs, build output and lifecycle history
//!   serve *understanding*; this serves *security*. Merging them would drown the
//!   signal.
//! - **Redacted on write.** [`Redactor`] runs before an entry is constructed, so
//!   a secret cannot enter the record even if a caller put one in a reason
//!   string. A display-time filter is not a control.
//! - **Secret access is recorded; secret values are not.** No [`AuditEvent`]
//!   variant has a field to put one in.
//!
//! ## What this does not claim
//!
//! Tamper **evidence**, not tamper **resistance**. An attacker with write access
//! to the file and the ability to run our own hashing code can rebuild a
//! consistent chain. Saying otherwise would be worse than saying nothing;
//! signing plus off-host verification is the planned upgrade, and is recorded as
//! such in [ADR-0010].
//!
//! [ADR-0010]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0010-hash-chained-audit-log.md
//!
//! # Example
//!
//! ```
//! use ephemeral_core::{Actor, AppId, Principal};
//! use ephemeral_core::audit::{AuditEvent, AuditLog};
//! use ephemeral_core::permission::{AppPermission, Decision, PathScope, Permission};
//!
//! let mut log = AuditLog::new();
//! let app = AppId::parse("apartment-comparator")?;
//!
//! log.append(
//!     Actor::User,
//!     AuditEvent::PermissionDecided {
//!         principal: Principal::app(app.clone()),
//!         permission: Permission::App(AppPermission::read(
//!             PathScope::parse("~/Downloads/apartments/**")?,
//!         )),
//!         decision: Decision::Allow,
//!     },
//! );
//!
//! assert!(log.verify().is_ok());
//! assert!(log.entries()[0].explain().contains("allowed"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod entry;
mod redact;

pub use entry::{AuditEntry, AuditEvent};
pub use redact::{MIN_SECRET_LENGTH, REDACTED, Redactor};

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Timestamp, actor::Actor, identity::AppId};

/// Why the audit record could not be trusted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AuditError {
    /// An entry's contents no longer match its recorded hash.
    ///
    /// Something changed a fact that had already been written down.
    #[error(
        "audit entry {sequence} has been altered: its contents no longer match its \
         recorded hash"
    )]
    EntryAltered {
        /// Which entry.
        sequence: u64,
    },

    /// An entry does not link to the one before it.
    ///
    /// What an insertion or a removal looks like from inside the chain.
    #[error("audit entry {sequence} does not follow the entry before it: the chain is broken")]
    ChainBroken {
        /// Which entry.
        sequence: u64,
    },

    /// Entries are not numbered consecutively from zero.
    #[error("audit entry at position {position} is numbered {sequence}: entries are missing")]
    SequenceGap {
        /// Where in the file.
        position: usize,
        /// What it claimed to be.
        sequence: u64,
    },
}

/// The append-only record of security-sensitive operations.
///
/// Holds a [`Redactor`], which is applied to every entry on the way in. The
/// redactor is not serialised: it holds secret values, and the whole point is
/// that those are never written down.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,

    #[serde(skip)]
    redactor: Redactor,
}

impl AuditLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty log that knows about specific secrets.
    #[must_use]
    pub fn with_redactor(redactor: Redactor) -> Self {
        Self {
            entries: Vec::new(),
            redactor,
        }
    }

    /// Registers a value that must never appear in the record.
    ///
    /// Returns whether it was registered; see [`Redactor::register_secret`].
    pub fn register_secret(&mut self, secret: impl Into<String>) -> bool {
        self.redactor.register_secret(secret)
    }

    /// Records something that happened.
    ///
    /// The event is redacted, then sealed into the chain. Returns the entry as
    /// it was written — which is not necessarily what was passed in, since
    /// redaction may have changed it.
    pub fn append(&mut self, actor: Actor, event: AuditEvent) -> &AuditEntry {
        self.append_with(actor, event, BTreeMap::new())
    }

    /// Records something that happened, with extra context.
    ///
    /// Detail values are redacted like everything else.
    pub fn append_with(
        &mut self,
        actor: Actor,
        event: AuditEvent,
        detail: BTreeMap<String, String>,
    ) -> &AuditEntry {
        let mut event = event;
        event.redact(&self.redactor);

        let detail = detail
            .into_iter()
            .map(|(key, value)| (key, self.redactor.redact(&value)))
            .collect();

        let previous_hash = self
            .entries
            .last()
            .map_or_else(String::new, |entry| entry.hash.clone());

        // A log long enough to overflow u64 is not reachable in this universe;
        // saturating keeps the conversion total without an unwrap.
        let sequence = u64::try_from(self.entries.len()).unwrap_or(u64::MAX);
        let entry = AuditEntry::seal(sequence, actor, event, detail, previous_hash);
        self.entries.push(entry);

        self.entries
            .last()
            .unwrap_or_else(|| unreachable!("an entry was just appended"))
    }

    /// Every entry, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// How many entries there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry concerning one application.
    #[must_use]
    pub fn entries_for(&self, app: &AppId) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.event.app() == Some(app))
            .collect()
    }

    /// Every entry recorded at or after a moment.
    #[must_use]
    pub fn entries_since(&self, since: Timestamp) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.at >= since)
            .collect()
    }

    /// Checks that the record has not been tampered with.
    ///
    /// Recomputes every hash and every link. Exposed through `ephemeral doctor`,
    /// because a verification failure is a security event a user should be told
    /// about, not a warning in a file nobody reads.
    ///
    /// # Errors
    ///
    /// The first [`AuditError`] found, naming the entry it was found at.
    pub fn verify(&self) -> Result<(), AuditError> {
        let mut expected_previous = String::new();

        for (position, entry) in self.entries.iter().enumerate() {
            if entry.sequence != u64::try_from(position).unwrap_or(u64::MAX) {
                return Err(AuditError::SequenceGap {
                    position,
                    sequence: entry.sequence,
                });
            }
            if entry.previous_hash != expected_previous {
                return Err(AuditError::ChainBroken {
                    sequence: entry.sequence,
                });
            }
            if !entry.is_intact() {
                return Err(AuditError::EntryAltered {
                    sequence: entry.sequence,
                });
            }
            expected_previous.clone_from(&entry.hash);
        }

        Ok(())
    }

    /// The hash of the most recent entry.
    ///
    /// Worth copying somewhere else: an independently held head hash turns
    /// tamper *evidence* into something an attacker with local write access
    /// cannot paper over, which is the limitation [ADR-0010] is candid about.
    ///
    /// [ADR-0010]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0010-hash-chained-audit-log.md
    #[must_use]
    pub fn head_hash(&self) -> Option<&str> {
        self.entries.last().map(|entry| entry.hash.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        identity::Principal,
        lifecycle::{LifecycleEvent, LifecycleState},
        permission::{AppPermission, Decision, PathScope, Permission},
    };

    fn app(id: &str) -> AppId {
        AppId::parse(id).unwrap()
    }

    fn granted(id: &str) -> AuditEvent {
        AuditEvent::PermissionDecided {
            principal: Principal::app(app(id)),
            permission: Permission::App(AppPermission::read(
                PathScope::parse("~/Downloads/apartments/**").unwrap(),
            )),
            decision: Decision::Allow,
        }
    }

    /// A log resembling a real session: an app created, a permission asked for
    /// and granted, a sandbox started, then torn down.
    fn a_session() -> AuditLog {
        let mut log = AuditLog::new();
        let id = app("csv-comparator");

        log.append(
            Actor::User,
            AuditEvent::AppCreated {
                app: id.clone(),
                purpose: "Compare the two listing exports I downloaded.".to_owned(),
            },
        );
        log.append(
            Actor::Ephemeral,
            AuditEvent::PermissionRequested {
                principal: Principal::app(id.clone()),
                permission: Permission::App(AppPermission::read(
                    PathScope::parse("~/Downloads/apartments/**").unwrap(),
                )),
                reason: "to compare the CSV files you selected".to_owned(),
            },
        );
        log.append(Actor::User, granted("csv-comparator"));
        log.append(
            Actor::Ephemeral,
            AuditEvent::SandboxCreated {
                app: id.clone(),
                runtime: "docker".to_owned(),
                image: Some("python:3.12-slim".to_owned()),
                mounts: vec!["~/Downloads/apartments".to_owned()],
                ports: vec![8080],
            },
        );
        log.append(
            Actor::Runtime,
            AuditEvent::LifecycleTransition {
                app: id,
                from: LifecycleState::Starting,
                to: LifecycleState::Running,
                event: LifecycleEvent::Started,
                reason: "the container reported healthy".to_owned(),
            },
        );
        log
    }

    // --- the chain ------------------------------------------------------------

    #[test]
    fn an_empty_log_verifies() {
        let log = AuditLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert_eq!(log.head_hash(), None);
        log.verify().unwrap();
    }

    #[test]
    fn a_well_formed_chain_verifies() {
        let log = a_session();
        assert_eq!(log.len(), 5);
        log.verify().unwrap();

        for (position, entry) in log.entries().iter().enumerate() {
            assert_eq!(entry.sequence, position as u64);
            assert!(entry.is_intact());
        }
    }

    #[test]
    fn each_entry_links_to_the_one_before_it() {
        let log = a_session();
        assert_eq!(log.entries()[0].previous_hash, "");

        for pair in log.entries().windows(2) {
            assert_eq!(
                pair[1].previous_hash, pair[0].hash,
                "entry {} does not link to {}",
                pair[1].sequence, pair[0].sequence
            );
        }
        assert_eq!(log.head_hash(), Some(log.entries()[4].hash.as_str()));
    }

    // --- tamper detection: the property this exists for ------------------------

    /// Flipping a decision after the fact is the exact attack the chain is
    /// meant to make visible.
    #[test]
    fn editing_an_entry_is_detected() {
        let mut log = a_session();

        log.entries[2].event = AuditEvent::PermissionDecided {
            principal: Principal::app(app("csv-comparator")),
            permission: Permission::App(AppPermission::read(PathScope::parse("~/**").unwrap())),
            decision: Decision::Allow,
        };

        assert_eq!(log.verify(), Err(AuditError::EntryAltered { sequence: 2 }));
    }

    #[test]
    fn changing_who_did_something_is_detected() {
        let mut log = a_session();
        log.entries[2].actor = Actor::Agent;
        assert_eq!(log.verify(), Err(AuditError::EntryAltered { sequence: 2 }));
    }

    /// Removing an inconvenient entry breaks both the numbering and the links.
    #[test]
    fn removing_an_entry_is_detected() {
        let mut log = a_session();
        log.entries.remove(2);

        assert!(matches!(
            log.verify(),
            Err(AuditError::SequenceGap { position: 2, .. })
        ));
    }

    #[test]
    fn truncating_the_chain_from_the_middle_is_detected() {
        let mut log = a_session();
        log.entries.truncate(3);
        log.entries.remove(0);

        assert!(log.verify().is_err(), "a removed head must be detectable");
    }

    #[test]
    fn reordering_entries_is_detected() {
        let mut log = a_session();
        log.entries.swap(1, 2);
        assert!(log.verify().is_err());
    }

    /// Inserting a fabricated entry cannot produce a consistent chain without
    /// recomputing everything after it.
    #[test]
    fn inserting_a_fabricated_entry_is_detected() {
        let mut log = a_session();
        let fabricated = AuditEntry::seal(
            2,
            Actor::User,
            granted("some-other-app"),
            BTreeMap::new(),
            log.entries[1].hash.clone(),
        );
        log.entries.insert(2, fabricated);

        assert!(log.verify().is_err());
    }

    /// Truncating from the end is the one thing a hash chain cannot see, and
    /// pretending otherwise would be dishonest. An independently held head hash
    /// is what closes it, which is why [`AuditLog::head_hash`] exists.
    #[test]
    fn truncation_from_the_end_needs_an_external_head_hash_to_detect() {
        let full = a_session();
        let head_before = full.head_hash().unwrap().to_owned();

        let mut truncated = full;
        truncated.entries.truncate(3);

        assert!(
            truncated.verify().is_ok(),
            "a truncated chain is internally consistent — this is the documented limit"
        );
        assert_ne!(
            truncated.head_hash().unwrap(),
            head_before,
            "but a head hash held elsewhere reveals it"
        );
    }

    // --- redaction on the write path -------------------------------------------

    /// A secret in a reason string must not reach the record, even though the
    /// caller put it there.
    #[test]
    fn secrets_are_redacted_before_an_entry_is_sealed() {
        let mut log = AuditLog::new();
        log.register_secret("a-registered-api-key");

        let entry = log.append(
            Actor::Ephemeral,
            AuditEvent::AppCreated {
                app: app("leaky"),
                purpose: "call the API with a-registered-api-key and DB_PASSWORD=hunter2"
                    .to_owned(),
            },
        );

        let json = serde_json::to_string(entry).unwrap();
        assert!(!json.contains("a-registered-api-key"), "{json}");
        assert!(!json.contains("hunter2"), "{json}");
        assert!(
            json.contains("call the API"),
            "the rest must survive: {json}"
        );
    }

    #[test]
    fn detail_values_are_redacted_too() {
        let mut log = AuditLog::new();
        log.register_secret("a-registered-api-key");

        let entry = log.append_with(
            Actor::Ephemeral,
            AuditEvent::AppPurged { app: app("leaky") },
            BTreeMap::from([
                (
                    "command".to_owned(),
                    "curl -H a-registered-api-key".to_owned(),
                ),
                ("exit_code".to_owned(), "0".to_owned()),
            ]),
        );

        assert!(!entry.detail["command"].contains("a-registered-api-key"));
        assert_eq!(entry.detail["exit_code"], "0");
    }

    /// Redaction happens before sealing, so a redacted entry is still a valid
    /// link in the chain rather than one that fails verification.
    #[test]
    fn a_redacted_entry_still_verifies() {
        let mut log = a_session();
        log.register_secret("a-registered-api-key");
        log.append(
            Actor::Agent,
            AuditEvent::AppCreated {
                app: app("leaky"),
                purpose: "using a-registered-api-key".to_owned(),
            },
        );

        log.verify().unwrap();
    }

    /// The redactor holds secret values, so it must never be serialised with
    /// the log.
    #[test]
    fn the_redactor_is_never_written_to_disk() {
        let mut log = AuditLog::new();
        log.register_secret("a-registered-api-key");
        log.append(Actor::User, granted("csv-comparator"));

        let json = serde_json::to_string(&log).unwrap();
        assert!(!json.contains("a-registered-api-key"), "{json}");
        assert!(!json.contains("redactor"), "{json}");
    }

    // --- querying and persistence ------------------------------------------------

    #[test]
    fn entries_can_be_found_by_application() {
        let mut log = a_session();
        log.append(Actor::User, granted("another-app"));

        assert_eq!(log.entries_for(&app("csv-comparator")).len(), 5);
        assert_eq!(log.entries_for(&app("another-app")).len(), 1);
        assert_eq!(log.entries_for(&app("never-existed")).len(), 0);
    }

    #[test]
    fn entries_can_be_found_by_time() {
        let log = a_session();
        let start = log.entries()[0].at;

        assert_eq!(log.entries_since(start).len(), 5);
        assert_eq!(
            log.entries_since(start + chrono::Duration::hours(1)).len(),
            0
        );
    }

    #[test]
    fn a_log_round_trips_through_json_and_still_verifies() {
        let log = a_session();
        let json = serde_json::to_string(&log).unwrap();
        let restored: AuditLog = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.len(), log.len());
        restored.verify().unwrap();
        assert_eq!(restored.head_hash(), log.head_hash());
    }

    /// A restored log keeps verifying as new entries are appended, so the chain
    /// survives a restart.
    #[test]
    fn a_restored_log_can_be_appended_to() {
        let json = serde_json::to_string(&a_session()).unwrap();
        let mut restored: AuditLog = serde_json::from_str(&json).unwrap();

        restored.append(Actor::User, granted("csv-comparator"));

        assert_eq!(restored.len(), 6);
        restored.verify().unwrap();
    }
}
