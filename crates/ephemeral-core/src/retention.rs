//! How ephemeral an application is.
//!
//! This is the module the product is named after. Deletion is not cleanup code
//! written at the end — it is a declared property of every application, chosen
//! when the app is created and changeable by the user at any time.
//!
//! If apps accumulate, Ephemeral becomes the thing it was built to replace: a
//! machine full of software nobody chose to keep.
//!
//! | Policy | Behaviour |
//! |--------|-----------|
//! | [`RetentionPolicy::OneShot`] | created, run, deleted |
//! | [`RetentionPolicy::Ephemeral`] | expires quickly (24h by default) |
//! | [`RetentionPolicy::Temporary`] | stays dormant, then expires (7d by default) |
//! | [`RetentionPolicy::Reusable`] | available until explicitly archived |
//! | [`RetentionPolicy::Persistent`] | behaves like a conventional application |
//!
//! Expiry archives rather than destroys, and deletion is recoverable for
//! [`DEFAULT_TRASH_PERIOD`] unless the user purges explicitly. An autonomous
//! system that irreversibly deletes a person's data on its own schedule is not
//! one they should trust ([ADR-0009]).
//!
//! [ADR-0009]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0009-storage-layout-and-retention.md

use std::fmt;

use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::Timestamp;

/// How long a deleted application stays recoverable before it can be purged.
pub const DEFAULT_TRASH_PERIOD: RetentionPeriod = RetentionPeriod::days(30);

/// The default lifetime of an [`RetentionPolicy::Ephemeral`] application.
pub const DEFAULT_EPHEMERAL_PERIOD: RetentionPeriod = RetentionPeriod::hours(24);

/// The default lifetime of a [`RetentionPolicy::Temporary`] application.
pub const DEFAULT_TEMPORARY_PERIOD: RetentionPeriod = RetentionPeriod::days(7);

/// A span of time, written the way a person would write it.
///
/// Parses and prints as `30s`, `15m`, `24h`, `7d` or `2w`. A plain number is
/// read as seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetentionPeriod {
    seconds: i64,
}

impl RetentionPeriod {
    /// A period of some number of seconds.
    #[must_use]
    pub const fn seconds(seconds: i64) -> Self {
        Self { seconds }
    }

    /// A period of some number of hours.
    #[must_use]
    pub const fn hours(hours: i64) -> Self {
        Self {
            seconds: hours * 3600,
        }
    }

    /// A period of some number of days.
    #[must_use]
    pub const fn days(days: i64) -> Self {
        Self {
            seconds: days * 86_400,
        }
    }

    /// The period in seconds.
    #[must_use]
    pub const fn as_seconds(self) -> i64 {
        self.seconds
    }

    /// The period as a [`chrono::Duration`].
    #[must_use]
    pub fn as_duration(self) -> Duration {
        Duration::seconds(self.seconds)
    }

    /// Parses a written period such as `24h` or `7d`.
    ///
    /// # Errors
    ///
    /// Returns [`RetentionError::UnparseablePeriod`] if the text is not a number
    /// followed by an optional unit, or [`RetentionError::NonPositivePeriod`] if
    /// the result is zero or negative — a retention period of zero would mean
    /// "delete immediately", which is [`RetentionPolicy::OneShot`] and should be
    /// said that way.
    pub fn parse(text: impl AsRef<str>) -> Result<Self, RetentionError> {
        let text = text.as_ref().trim();
        let unparseable = || RetentionError::UnparseablePeriod {
            text: text.to_owned(),
        };

        let (number, multiplier) = match text.chars().last() {
            Some('s') => (&text[..text.len() - 1], 1),
            Some('m') => (&text[..text.len() - 1], 60),
            Some('h') => (&text[..text.len() - 1], 3_600),
            Some('d') => (&text[..text.len() - 1], 86_400),
            Some('w') => (&text[..text.len() - 1], 604_800),
            Some(c) if c.is_ascii_digit() => (text, 1),
            _ => return Err(unparseable()),
        };

        let value: i64 = number.trim().parse().map_err(|_| unparseable())?;
        let seconds = value.checked_mul(multiplier).ok_or_else(unparseable)?;

        if seconds <= 0 {
            return Err(RetentionError::NonPositivePeriod {
                text: text.to_owned(),
            });
        }

        Ok(Self { seconds })
    }

    /// The period written the way it would be typed.
    ///
    /// Uses the largest unit that divides evenly, so 86 400 seconds prints as
    /// `1d` rather than `86400s`.
    #[must_use]
    pub fn as_written(self) -> String {
        for (unit, size) in [('w', 604_800), ('d', 86_400), ('h', 3_600), ('m', 60)] {
            if self.seconds % size == 0 {
                return format!("{}{unit}", self.seconds / size);
            }
        }
        format!("{}s", self.seconds)
    }

    /// The period in language a person would use, such as "24 hours".
    #[must_use]
    pub fn describe(self) -> String {
        let plural = |count: i64, unit: &str| {
            if count == 1 {
                format!("1 {unit}")
            } else {
                format!("{count} {unit}s")
            }
        };

        for (unit, size) in [
            ("week", 604_800),
            ("day", 86_400),
            ("hour", 3_600),
            ("minute", 60),
        ] {
            if self.seconds % size == 0 {
                return plural(self.seconds / size, unit);
            }
        }
        plural(self.seconds, "second")
    }
}

impl fmt::Display for RetentionPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_written())
    }
}

impl std::str::FromStr for RetentionPeriod {
    type Err = RetentionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for RetentionPeriod {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_written())
    }
}

impl<'de> Deserialize<'de> for RetentionPeriod {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let written = String::deserialize(deserializer)?;
        Self::parse(&written).map_err(serde::de::Error::custom)
    }
}

/// Why a retention value was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RetentionError {
    /// The period was not a number followed by an optional unit.
    #[error("{text:?} is not a period; write something like 30m, 24h, 7d or 2w")]
    UnparseablePeriod {
        /// The text that was rejected.
        text: String,
    },

    /// The period was zero or negative.
    #[error(
        "{text:?} is not a positive period; for an app that is deleted after one run, \
             use the one-shot policy instead"
    )]
    NonPositivePeriod {
        /// The text that was rejected.
        text: String,
    },
}

/// How long an application should stick around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum RetentionPolicy {
    /// Created, run once, then deleted.
    ///
    /// For the app you needed for ten minutes and will never think about again.
    OneShot,

    /// Expires soon after it stops being used.
    Ephemeral {
        /// How long. Defaults to [`DEFAULT_EPHEMERAL_PERIOD`].
        #[serde(default = "default_ephemeral_period")]
        retain_for: RetentionPeriod,
    },

    /// Stays available but dormant, then expires.
    Temporary {
        /// How long. Defaults to [`DEFAULT_TEMPORARY_PERIOD`].
        #[serde(default = "default_temporary_period")]
        retain_for: RetentionPeriod,
    },

    /// Available until the user explicitly archives it.
    Reusable,

    /// Behaves like a conventional installed application.
    Persistent,
}

fn default_ephemeral_period() -> RetentionPeriod {
    DEFAULT_EPHEMERAL_PERIOD
}

fn default_temporary_period() -> RetentionPeriod {
    DEFAULT_TEMPORARY_PERIOD
}

impl Default for RetentionPolicy {
    /// [`RetentionPolicy::Temporary`], with the default period.
    ///
    /// A week is long enough that a useful app is still there when the user
    /// comes back to it, and short enough that the machine does not silently
    /// fill up with things nobody chose to keep. Users who want either extreme
    /// have four other policies.
    fn default() -> Self {
        Self::Temporary {
            retain_for: DEFAULT_TEMPORARY_PERIOD,
        }
    }
}

impl RetentionPolicy {
    /// Every policy, with default periods, for interfaces that offer a choice.
    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::OneShot,
            Self::Ephemeral {
                retain_for: DEFAULT_EPHEMERAL_PERIOD,
            },
            Self::Temporary {
                retain_for: DEFAULT_TEMPORARY_PERIOD,
            },
            Self::Reusable,
            Self::Persistent,
        ]
    }

    /// The machine-readable policy name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::OneShot => "one_shot",
            Self::Ephemeral { .. } => "ephemeral",
            Self::Temporary { .. } => "temporary",
            Self::Reusable => "reusable",
            Self::Persistent => "persistent",
        }
    }

    /// How long this application is kept, if it expires by itself.
    #[must_use]
    pub fn retain_for(self) -> Option<RetentionPeriod> {
        match self {
            Self::Ephemeral { retain_for } | Self::Temporary { retain_for } => Some(retain_for),
            Self::OneShot | Self::Reusable | Self::Persistent => None,
        }
    }

    /// Whether this application should be deleted as soon as it has run.
    #[must_use]
    pub fn deletes_after_running(self) -> bool {
        matches!(self, Self::OneShot)
    }

    /// Whether this application ever expires on its own.
    #[must_use]
    pub fn expires_on_its_own(self) -> bool {
        self.retain_for().is_some()
    }

    /// When this application expires, given when it was last used.
    ///
    /// `None` means it never expires by itself. A one-shot app has no expiry
    /// time either: it is deleted by having run, not by the clock.
    #[must_use]
    pub fn expires_at(self, last_used: Timestamp) -> Option<Timestamp> {
        self.retain_for()
            .map(|period| last_used + period.as_duration())
    }

    /// Whether this application has expired.
    #[must_use]
    pub fn has_expired(self, last_used: Timestamp, now: Timestamp) -> bool {
        self.expires_at(last_used)
            .is_some_and(|expiry| now >= expiry)
    }

    /// A short label for a UI.
    #[must_use]
    pub fn headline(self) -> String {
        match self {
            Self::OneShot => "Use once".to_owned(),
            Self::Ephemeral { retain_for } | Self::Temporary { retain_for } => {
                format!("Keep for {}", retain_for.describe())
            }
            Self::Reusable => "Keep until I archive it".to_owned(),
            Self::Persistent => "Keep indefinitely".to_owned(),
        }
    }

    /// What this policy means, in plain language.
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::OneShot => {
                "This app is deleted as soon as it has done its job. Nothing is kept.".to_owned()
            }
            Self::Ephemeral { retain_for } => format!(
                "This app is archived {} after you last use it, then deleted. You can \
                 restore it until it is purged.",
                retain_for.describe()
            ),
            Self::Temporary { retain_for } => format!(
                "This app stays available for {} after you last use it, then is archived \
                 and eventually deleted.",
                retain_for.describe()
            ),
            Self::Reusable => {
                "This app stays available until you archive it. It uses nothing while it \
                 is not running."
                    .to_owned()
            }
            Self::Persistent => {
                "This app behaves like an app you installed. It stays until you delete it."
                    .to_owned()
            }
        }
    }
}

impl fmt::Display for RetentionPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.retain_for() {
            Some(period) => write!(f, "{}({period})", self.name()),
            None => f.write_str(self.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::now;

    // --- periods -------------------------------------------------------------

    #[test]
    fn periods_parse_every_unit() {
        assert_eq!(RetentionPeriod::parse("45s").unwrap().as_seconds(), 45);
        assert_eq!(RetentionPeriod::parse("15m").unwrap().as_seconds(), 900);
        assert_eq!(RetentionPeriod::parse("24h").unwrap().as_seconds(), 86_400);
        assert_eq!(RetentionPeriod::parse("7d").unwrap().as_seconds(), 604_800);
        assert_eq!(
            RetentionPeriod::parse("2w").unwrap().as_seconds(),
            1_209_600
        );
        assert_eq!(
            RetentionPeriod::parse("90").unwrap().as_seconds(),
            90,
            "a bare number should be read as seconds"
        );
    }

    #[test]
    fn periods_print_in_the_largest_whole_unit() {
        assert_eq!(RetentionPeriod::hours(24).as_written(), "1d");
        assert_eq!(RetentionPeriod::days(7).as_written(), "1w");
        assert_eq!(RetentionPeriod::hours(25).as_written(), "25h");
        assert_eq!(RetentionPeriod::seconds(90).as_written(), "90s");
    }

    #[test]
    fn periods_describe_themselves_in_words() {
        assert_eq!(RetentionPeriod::hours(24).describe(), "1 day");
        assert_eq!(RetentionPeriod::days(7).describe(), "1 week");
        assert_eq!(RetentionPeriod::hours(3).describe(), "3 hours");
        assert_eq!(RetentionPeriod::seconds(1).describe(), "1 second");
    }

    /// A zero or negative retention period would mean "delete immediately",
    /// which is a policy of its own and should be said that way rather than
    /// smuggled in as a duration.
    #[test]
    fn periods_must_be_positive_and_well_formed() {
        assert!(matches!(
            RetentionPeriod::parse("0h"),
            Err(RetentionError::NonPositivePeriod { .. })
        ));
        assert!(matches!(
            RetentionPeriod::parse("-1d"),
            Err(RetentionError::NonPositivePeriod { .. })
        ));
        for bad in ["", "soon", "7 days", "d", "1y", "1.5h"] {
            assert!(
                matches!(
                    RetentionPeriod::parse(bad),
                    Err(RetentionError::UnparseablePeriod { .. })
                ),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn periods_round_trip_through_yaml() {
        for written in ["45s", "15m", "3h", "2d", "1w"] {
            let parsed = RetentionPeriod::parse(written).unwrap();
            let yaml = serde_norway::to_string(&parsed).unwrap();
            assert_eq!(
                serde_norway::from_str::<RetentionPeriod>(&yaml).unwrap(),
                parsed
            );
        }
    }

    // --- policies ------------------------------------------------------------

    #[test]
    fn the_default_policy_is_temporary_for_a_week() {
        assert_eq!(
            RetentionPolicy::default(),
            RetentionPolicy::Temporary {
                retain_for: DEFAULT_TEMPORARY_PERIOD
            }
        );
        assert_eq!(DEFAULT_TEMPORARY_PERIOD.as_written(), "1w");
    }

    #[test]
    fn only_the_timed_policies_expire_on_their_own() {
        let expiring = [
            RetentionPolicy::Ephemeral {
                retain_for: DEFAULT_EPHEMERAL_PERIOD,
            },
            RetentionPolicy::Temporary {
                retain_for: DEFAULT_TEMPORARY_PERIOD,
            },
        ];
        for policy in expiring {
            assert!(policy.expires_on_its_own(), "{policy} should expire");
        }
        for policy in [
            RetentionPolicy::OneShot,
            RetentionPolicy::Reusable,
            RetentionPolicy::Persistent,
        ] {
            assert!(!policy.expires_on_its_own(), "{policy} should not expire");
            assert_eq!(policy.expires_at(now()), None);
        }
    }

    #[test]
    fn only_one_shot_deletes_after_running() {
        assert!(RetentionPolicy::OneShot.deletes_after_running());
        for policy in RetentionPolicy::all()
            .into_iter()
            .filter(|p| *p != RetentionPolicy::OneShot)
        {
            assert!(!policy.deletes_after_running(), "{policy} should not");
        }
    }

    #[test]
    fn expiry_is_measured_from_last_use() {
        let policy = RetentionPolicy::Ephemeral {
            retain_for: RetentionPeriod::hours(24),
        };
        let last_used = now();

        assert!(!policy.has_expired(last_used, last_used));
        assert!(!policy.has_expired(last_used, last_used + Duration::hours(23)));
        assert!(policy.has_expired(last_used, last_used + Duration::hours(24)));
        assert!(policy.has_expired(last_used, last_used + Duration::days(3)));
    }

    #[test]
    fn a_policy_that_never_expires_never_expires() {
        let long_ago = now() - Duration::days(3650);
        for policy in [RetentionPolicy::Persistent, RetentionPolicy::Reusable] {
            assert!(
                !policy.has_expired(long_ago, now()),
                "{policy} must not expire"
            );
        }
    }

    #[test]
    fn every_policy_explains_itself() {
        for policy in RetentionPolicy::all() {
            assert!(!policy.headline().is_empty(), "{policy} has no headline");
            assert!(
                policy.describe().len() > 30,
                "{policy} needs a real explanation"
            );
            assert!(!policy.name().is_empty());
        }
    }

    #[test]
    fn policies_round_trip_through_yaml() {
        for policy in RetentionPolicy::all() {
            let yaml = serde_norway::to_string(&policy).unwrap();
            assert_eq!(
                serde_norway::from_str::<RetentionPolicy>(&yaml).unwrap(),
                policy,
                "{yaml} did not round-trip"
            );
        }
    }

    /// Writing a policy without its period should get the documented default
    /// rather than an error, so a hand-written manifest stays short.
    #[test]
    fn a_policy_without_a_period_gets_the_default() {
        let parsed: RetentionPolicy = serde_norway::from_str("policy: ephemeral\n").unwrap();
        assert_eq!(
            parsed,
            RetentionPolicy::Ephemeral {
                retain_for: DEFAULT_EPHEMERAL_PERIOD
            }
        );

        let parsed: RetentionPolicy =
            serde_norway::from_str("policy: temporary\nretain_for: 3d\n").unwrap();
        assert_eq!(
            parsed,
            RetentionPolicy::Temporary {
                retain_for: RetentionPeriod::days(3)
            }
        );
    }

    #[test]
    fn the_trash_period_gives_people_time_to_change_their_mind() {
        assert!(
            DEFAULT_TRASH_PERIOD.as_seconds() >= RetentionPeriod::days(7).as_seconds(),
            "a recovery window shorter than a week is not a recovery window"
        );
    }
}
