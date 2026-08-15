//! What goes into the audit record.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Redactor;
use crate::{
    Timestamp,
    actor::Actor,
    identity::{AppId, Principal},
    lifecycle::{LifecycleEvent, LifecycleState},
    now,
    permission::{Decision, Permission},
};

/// A security-relevant thing that happened.
///
/// Deliberately a closed set rather than a free-text message. An audit log made
/// of prose cannot be queried, aggregated, or checked for coverage; a closed set
/// can, and makes "which security events do we record?" an answerable question.
///
/// Note what is *not* here: no variant carries a secret value. Secret **access**
/// is recorded ([`AuditEvent::SecretAccessed`]); secret values are not, and
/// there is no field to put one in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// Tagged `kind` rather than `event`, because one variant already has a
// field called `event` (the lifecycle event that caused a transition).
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditEvent {
    /// Something asked for a permission.
    PermissionRequested {
        /// Who would hold it.
        principal: Principal,
        /// What was asked for.
        permission: Permission,
        /// Why, in the user's terms.
        reason: String,
    },

    /// A person decided a permission question.
    PermissionDecided {
        /// Who holds it.
        principal: Principal,
        /// What was decided about.
        permission: Permission,
        /// What was decided.
        decision: Decision,
    },

    /// A permission was taken back.
    PermissionRevoked {
        /// Who held it.
        principal: Principal,
        /// What was revoked.
        permission: Permission,
    },

    /// An application moved through its lifecycle.
    LifecycleTransition {
        /// Which application.
        app: AppId,
        /// The state before.
        from: LifecycleState,
        /// The state after.
        to: LifecycleState,
        /// What caused it.
        event: LifecycleEvent,
        /// Why, in the user's terms.
        reason: String,
    },

    /// An application was created.
    AppCreated {
        /// Which application.
        app: AppId,
        /// What the user asked for.
        purpose: String,
    },

    /// An application was deleted: its runtime resources destroyed and its
    /// permissions revoked.
    AppDeleted {
        /// Which application.
        app: AppId,
        /// How many grants were withdrawn.
        grants_revoked: usize,
    },

    /// An application's data was removed irreversibly.
    AppPurged {
        /// Which application.
        app: AppId,
    },

    /// A sandbox was created for an application.
    ///
    /// Records exactly what was exposed to it, because that is the question an
    /// incident review asks first.
    SandboxCreated {
        /// Which application.
        app: AppId,
        /// What it runs on.
        runtime: String,
        /// The image, if containerised.
        image: Option<String>,
        /// Every host path mounted into it.
        mounts: Vec<String>,
        /// Every port published from it.
        ports: Vec<u16>,
    },

    /// A sandbox was destroyed.
    SandboxDestroyed {
        /// Which application.
        app: AppId,
        /// Why it went away.
        reason: String,
    },

    /// An application was written out for somebody else to build.
    ///
    /// Publishing is an outbound data flow, so it is recorded like one: what
    /// left, and where it went.
    AppPublished {
        /// Which application.
        app: AppId,
        /// Where it was written.
        destination: String,
    },

    /// An application somebody else published was accepted.
    AppInstalled {
        /// The id it was given here. A new installation, not the sender's.
        app: AppId,
        /// Where it came from.
        origin: String,
    },

    /// A principal read a secret.
    ///
    /// The **name** is recorded. The value is not, and cannot be: there is no
    /// field for it.
    SecretAccessed {
        /// Who read it.
        principal: Principal,
        /// Which secret, by name.
        name: String,
    },

    /// A generation run started.
    GenerationStarted {
        /// Which application.
        app: AppId,
        /// Which model provider, by name — never a credential.
        provider: String,
    },

    /// A generation run finished.
    GenerationFinished {
        /// Which application.
        app: AppId,
        /// Whether it produced a working application.
        succeeded: bool,
        /// How many repair attempts it took.
        repairs: u32,
    },

    /// Ephemeral's own configuration changed in a security-relevant way.
    SettingChanged {
        /// Which setting.
        setting: String,
        /// What it became. Never a secret value.
        value: String,
    },
}

impl AuditEvent {
    /// The stable event name, matching the serialised form.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::PermissionRequested { .. } => "permission_requested",
            Self::PermissionDecided { .. } => "permission_decided",
            Self::PermissionRevoked { .. } => "permission_revoked",
            Self::LifecycleTransition { .. } => "lifecycle_transition",
            Self::AppCreated { .. } => "app_created",
            Self::AppDeleted { .. } => "app_deleted",
            Self::AppPurged { .. } => "app_purged",
            Self::AppPublished { .. } => "app_published",
            Self::AppInstalled { .. } => "app_installed",
            Self::SandboxCreated { .. } => "sandbox_created",
            Self::SandboxDestroyed { .. } => "sandbox_destroyed",
            Self::SecretAccessed { .. } => "secret_accessed",
            Self::GenerationStarted { .. } => "generation_started",
            Self::GenerationFinished { .. } => "generation_finished",
            Self::SettingChanged { .. } => "setting_changed",
        }
    }

    /// The application this concerns, if it concerns one.
    #[must_use]
    pub fn app(&self) -> Option<&AppId> {
        match self {
            Self::LifecycleTransition { app, .. }
            | Self::AppCreated { app, .. }
            | Self::AppDeleted { app, .. }
            | Self::AppPurged { app }
            | Self::SandboxCreated { app, .. }
            | Self::SandboxDestroyed { app, .. }
            | Self::GenerationStarted { app, .. }
            | Self::GenerationFinished { app, .. }
            | Self::AppPublished { app, .. }
            | Self::AppInstalled { app, .. } => Some(app),
            Self::PermissionRequested { principal, .. }
            | Self::PermissionDecided { principal, .. }
            | Self::PermissionRevoked { principal, .. }
            | Self::SecretAccessed { principal, .. } => principal.as_app(),
            Self::SettingChanged { .. } => None,
        }
    }

    /// A plain-language account, for the activity view a user reads.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::PermissionRequested {
                principal,
                permission,
                ..
            } => format!("{} asked to {}", principal.label(), permission.describe()),
            Self::PermissionDecided {
                principal,
                permission,
                decision,
            } => {
                let verb = match decision {
                    Decision::Allow => "allowed",
                    Decision::Deny => "denied",
                };
                format!("{verb} {} to {}", principal.label(), permission.describe())
            }
            Self::PermissionRevoked {
                principal,
                permission,
            } => format!(
                "took back {}'s permission to {}",
                principal.label(),
                permission.describe()
            ),
            Self::LifecycleTransition { app, from, to, .. } => {
                format!("{app} moved from {from} to {to}")
            }
            Self::AppCreated { app, purpose } => {
                if purpose.is_empty() {
                    format!("created {app}")
                } else {
                    format!("created {app} to: {purpose}")
                }
            }
            Self::AppDeleted {
                app,
                grants_revoked,
            } => format!("deleted {app} and withdrew {grants_revoked} permission(s)"),
            Self::AppPurged { app } => format!("permanently removed {app} and all its data"),
            Self::SandboxCreated {
                app,
                runtime,
                mounts,
                ports,
                ..
            } => format!(
                "started {app} in a {runtime} sandbox with {} folder(s) and {} port(s)",
                mounts.len(),
                ports.len()
            ),
            Self::SandboxDestroyed { app, reason } => {
                format!("shut down {app}'s sandbox: {reason}")
            }
            Self::SecretAccessed { principal, name } => {
                format!("{} used the setting {name}", principal.label())
            }
            Self::GenerationStarted { app, provider } => {
                format!("started building {app} using {provider}")
            }
            Self::GenerationFinished {
                app,
                succeeded,
                repairs,
            } => {
                let outcome = if *succeeded { "finished" } else { "gave up on" };
                format!("{outcome} building {app} after {repairs} repair attempt(s)")
            }
            Self::AppPublished { app, destination } => {
                format!("published {app} to {destination}")
            }
            Self::AppInstalled { app, origin } => {
                format!("installed {app} from {origin}, with no permissions")
            }
            Self::SettingChanged { setting, value } => {
                format!("changed {setting} to {value}")
            }
        }
    }

    /// Removes anything the redactor recognises from this event's free-text
    /// fields.
    ///
    /// Applied by the log before an entry is built, so a secret cannot reach the
    /// record even if a caller put one in a reason string.
    pub fn redact(&mut self, redactor: &Redactor) {
        match self {
            Self::PermissionRequested { reason, .. }
            | Self::LifecycleTransition { reason, .. }
            | Self::SandboxDestroyed { reason, .. } => redactor.redact_in_place(reason),
            Self::AppCreated { purpose, .. } => redactor.redact_in_place(purpose),
            Self::SandboxCreated { mounts, image, .. } => {
                for mount in mounts.iter_mut() {
                    redactor.redact_in_place(mount);
                }
                if let Some(image) = image {
                    redactor.redact_in_place(image);
                }
            }
            Self::SettingChanged { value, .. } => redactor.redact_in_place(value),
            Self::GenerationStarted { provider, .. } => redactor.redact_in_place(provider),
            // A path can carry a secret — somebody's token in a directory name
            // is not hypothetical — so both ends of a share are redacted.
            Self::AppPublished { destination, .. } => redactor.redact_in_place(destination),
            Self::AppInstalled { origin, .. } => redactor.redact_in_place(origin),
            // The remaining variants carry only identifiers, names, enums and
            // counts. There is nowhere in them for a secret to hide.
            Self::PermissionDecided { .. }
            | Self::PermissionRevoked { .. }
            | Self::AppDeleted { .. }
            | Self::AppPurged { .. }
            | Self::SecretAccessed { .. }
            | Self::GenerationFinished { .. } => {}
        }
    }
}

impl fmt::Display for AuditEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One entry in the audit log.
///
/// Entries are appended and never modified. Each carries the hash of its
/// predecessor and a hash of itself, so any alteration, reordering or removal
/// within the chain is detectable by
/// [`AuditLog::verify`](super::AuditLog::verify).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Position in the chain, starting at 0.
    pub sequence: u64,

    /// When it happened, in UTC.
    pub at: Timestamp,

    /// Who did it.
    pub actor: Actor,

    /// What happened.
    pub event: AuditEvent,

    /// Extra context. Values are redacted on the way in.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub detail: BTreeMap<String, String>,

    /// The hash of the previous entry, or the empty string for the first.
    pub previous_hash: String,

    /// This entry's own hash, over every field above.
    pub hash: String,
}

impl AuditEntry {
    /// Builds an entry and seals it into the chain.
    ///
    /// Only [`AuditLog::append`](super::AuditLog::append) calls this: an entry
    /// that was not appended through the log has no place in the chain.
    pub(super) fn seal(
        sequence: u64,
        actor: Actor,
        event: AuditEvent,
        detail: BTreeMap<String, String>,
        previous_hash: String,
    ) -> Self {
        let mut entry = Self {
            sequence,
            at: now(),
            actor,
            event,
            detail,
            previous_hash,
            hash: String::new(),
        };
        entry.hash = entry.compute_hash();
        entry
    }

    /// Recomputes this entry's hash from its contents.
    ///
    /// The hash covers everything except the hash field itself, so changing any
    /// recorded fact — the actor, the time, the event, a detail value, or the
    /// link to the previous entry — changes it.
    #[must_use]
    pub fn compute_hash(&self) -> String {
        let mut hasher = Sha256::new();

        hasher.update(self.sequence.to_be_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.at.to_rfc3339().as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.actor.as_str().as_bytes());
        hasher.update(b"\x1f");

        // serde_json is deterministic for these types: struct fields serialise
        // in declaration order and the detail map is a BTreeMap, so the byte
        // sequence is stable across runs and platforms.
        let event = serde_json::to_string(&self.event)
            .unwrap_or_else(|_| format!("unserialisable:{}", self.event.name()));
        hasher.update(event.as_bytes());
        hasher.update(b"\x1f");

        for (key, value) in &self.detail {
            hasher.update(key.as_bytes());
            hasher.update(b"\x1e");
            hasher.update(value.as_bytes());
            hasher.update(b"\x1e");
        }
        hasher.update(b"\x1f");
        hasher.update(self.previous_hash.as_bytes());

        hex::encode(hasher.finalize())
    }

    /// Whether this entry's recorded hash still matches its contents.
    #[must_use]
    pub fn is_intact(&self) -> bool {
        self.hash == self.compute_hash()
    }

    /// A one-line account for a person: when, who, what.
    #[must_use]
    pub fn explain(&self) -> String {
        format!(
            "{}  {} {}",
            self.at.format("%Y-%m-%d %H:%M:%S"),
            self.actor.describe(),
            self.event.describe()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{AppPermission, PathScope};

    fn app() -> AppId {
        AppId::parse("csv-comparator").unwrap()
    }

    fn sample_event() -> AuditEvent {
        AuditEvent::PermissionDecided {
            principal: Principal::app(app()),
            permission: Permission::App(AppPermission::read(
                PathScope::parse("~/Downloads/apartments/**").unwrap(),
            )),
            decision: Decision::Allow,
        }
    }

    fn sealed(event: AuditEvent) -> AuditEntry {
        AuditEntry::seal(0, Actor::User, event, BTreeMap::new(), String::new())
    }

    #[test]
    fn events_describe_themselves_in_plain_language() {
        let described = sample_event().describe();
        assert!(described.contains("allowed"));
        assert!(described.contains("csv-comparator"));
        assert!(described.contains("~/Downloads/apartments"));
    }

    #[test]
    fn events_know_which_application_they_concern() {
        assert_eq!(sample_event().app(), Some(&app()));
        assert_eq!(AuditEvent::AppPurged { app: app() }.app(), Some(&app()));
        assert_eq!(
            AuditEvent::SettingChanged {
                setting: "theme".to_owned(),
                value: "dark".to_owned()
            }
            .app(),
            None
        );
    }

    #[test]
    fn a_sealed_entry_is_intact() {
        assert!(sealed(sample_event()).is_intact());
    }

    /// The property the chain exists for: any change to a recorded fact breaks
    /// the entry's hash.
    #[test]
    fn altering_any_recorded_fact_breaks_the_hash() {
        let original = sealed(sample_event());

        let mut actor_changed = original.clone();
        actor_changed.actor = Actor::Agent;
        assert!(
            !actor_changed.is_intact(),
            "a changed actor must be detectable"
        );

        let mut time_changed = original.clone();
        time_changed.at = now() + chrono::Duration::hours(1);
        assert!(
            !time_changed.is_intact(),
            "a changed time must be detectable"
        );

        let mut decision_changed = original.clone();
        decision_changed.event = AuditEvent::PermissionDecided {
            principal: Principal::app(app()),
            permission: Permission::App(AppPermission::read(
                PathScope::parse("~/Downloads/apartments/**").unwrap(),
            )),
            decision: Decision::Deny,
        };
        assert!(
            !decision_changed.is_intact(),
            "flipping a permission decision must be detectable"
        );

        let mut link_changed = original.clone();
        link_changed.previous_hash = "0".repeat(64);
        assert!(
            !link_changed.is_intact(),
            "a changed link must be detectable"
        );

        let mut detail_changed = original;
        detail_changed
            .detail
            .insert("added".to_owned(), "later".to_owned());
        assert!(
            !detail_changed.is_intact(),
            "an added detail must be detectable"
        );
    }

    /// Hashing must be stable, or verification would fail on entries nobody
    /// touched.
    #[test]
    fn hashing_the_same_contents_twice_gives_the_same_hash() {
        let entry = sealed(sample_event());
        assert_eq!(entry.compute_hash(), entry.compute_hash());
        assert_eq!(entry.hash.len(), 64, "a SHA-256 digest in hex");
    }

    /// Two entries that differ only in position must hash differently, so
    /// reordering is detectable.
    #[test]
    fn position_is_part_of_the_hash() {
        let first = AuditEntry::seal(
            0,
            Actor::User,
            sample_event(),
            BTreeMap::new(),
            String::new(),
        );
        let mut moved = first.clone();
        moved.sequence = 1;
        assert!(!moved.is_intact());
    }

    #[test]
    fn free_text_fields_are_redacted() {
        let mut redactor = Redactor::new();
        redactor.register_secret("a-registered-secret");

        let mut event = AuditEvent::AppCreated {
            app: app(),
            purpose: "compare the files, password=hunter2 and a-registered-secret".to_owned(),
        };
        event.redact(&redactor);

        let described = event.describe();
        assert!(!described.contains("hunter2"));
        assert!(!described.contains("a-registered-secret"));
        assert!(described.contains("compare the files"));
    }

    /// Mount paths are attacker-influenced and end up in the record, so they go
    /// through the redactor too.
    #[test]
    fn mount_paths_are_redacted() {
        let mut redactor = Redactor::new();
        redactor.register_secret("secret-project-name");

        let mut event = AuditEvent::SandboxCreated {
            app: app(),
            runtime: "docker".to_owned(),
            image: Some("registry/secret-project-name:1".to_owned()),
            mounts: vec!["~/work/secret-project-name".to_owned()],
            ports: vec![8080],
        };
        event.redact(&redactor);

        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("secret-project-name"), "{json}");
    }

    /// A secret's use is recorded; its value is not, and there is no field for
    /// one.
    #[test]
    fn secret_access_records_the_name_and_nothing_else() {
        let event = AuditEvent::SecretAccessed {
            principal: Principal::app(app()),
            name: "ANTHROPIC_API_KEY".to_owned(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("ANTHROPIC_API_KEY"));
        assert!(event.describe().contains("ANTHROPIC_API_KEY"));
        assert!(
            !json.contains("value"),
            "there must be no value field: {json}"
        );
    }

    #[test]
    fn entries_round_trip_through_json() {
        let entry = sealed(sample_event());
        let json = serde_json::to_string(&entry).unwrap();
        let restored: AuditEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, entry);
        assert!(
            restored.is_intact(),
            "a round trip must not break the chain"
        );
    }

    #[test]
    fn entries_explain_themselves_for_a_person() {
        let explanation = sealed(sample_event()).explain();
        assert!(explanation.contains("you allowed"), "{explanation}");
        assert!(
            explanation.contains("~/Downloads/apartments"),
            "{explanation}"
        );
    }
}
