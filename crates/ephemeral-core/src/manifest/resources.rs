//! Limits. What an application may consume, and what generating it may cost.
//!
//! Every limit here exists because an unbounded version of it is a security or
//! financial problem rather than merely an annoyance. A generated application
//! that spins the CPU is a denial of service against its own user; an
//! autonomous repair loop with no ceiling is a bill.
//!
//! The defaults are deliberately modest. An application that genuinely needs
//! more asks for more, and the user sees the ask.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::retention::RetentionPeriod;

/// What one application may consume while it runs.
///
/// Enforced by the runtime — the values here are what the runtime is told to
/// apply, and a runtime that cannot apply them must refuse to start the
/// application rather than run it unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    /// CPU, in thousandths of a core. 1000 is one core.
    pub cpu_millis: u32,

    /// Memory ceiling, in mebibytes.
    pub memory_mib: u32,

    /// Disk ceiling for the application's own storage, in mebibytes.
    pub storage_mib: u32,

    /// Maximum number of processes or threads.
    ///
    /// The limit that stops a fork bomb, whether deliberate or accidental.
    pub max_processes: u32,

    /// How long the application may run before it is stopped.
    ///
    /// `None` means no wall-clock limit, which is only appropriate for
    /// long-running services the user has explicitly asked to keep running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime: Option<RetentionPeriod>,
}

impl Default for ResourceLimits {
    /// Enough for the kind of small tool Ephemeral generates, and no more.
    ///
    /// Half a core, 512 MiB of memory, 1 GiB of disk, 64 processes, and a
    /// fifteen-minute wall clock. A CSV comparator does not need more; anything
    /// that does can ask.
    fn default() -> Self {
        Self {
            cpu_millis: 500,
            memory_mib: 512,
            storage_mib: 1024,
            max_processes: 64,
            max_runtime: Some(RetentionPeriod::seconds(900)),
        }
    }
}

impl ResourceLimits {
    /// Limits for a long-running service, which has no wall-clock ceiling.
    #[must_use]
    pub fn service() -> Self {
        Self {
            max_runtime: None,
            ..Self::default()
        }
    }

    /// Whether every limit is a usable value.
    ///
    /// A zero limit is refused rather than treated as "unlimited": a manifest
    /// that means unlimited has to say so by omitting the field, so nobody
    /// removes a ceiling by accident.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.cpu_millis > 0
            && self.memory_mib > 0
            && self.storage_mib > 0
            && self.max_processes > 0
            && self.max_runtime.is_none_or(|d| d.as_seconds() > 0)
    }

    /// A one-line summary for an app's detail page.
    #[must_use]
    pub fn describe(&self) -> String {
        let cpu = f64::from(self.cpu_millis) / 1000.0;
        let runtime = match self.max_runtime {
            Some(period) => format!(", stops after {}", period.describe()),
            None => String::new(),
        };
        format!(
            "up to {cpu:.2} CPU cores, {} MiB of memory, {} MiB of disk{runtime}",
            self.memory_mib, self.storage_mib
        )
    }
}

impl fmt::Display for ResourceLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// What building an application is allowed to cost.
///
/// The autonomous plan/generate/build/test/repair loop is bounded on every axis
/// it can consume, and the user can cancel it at any point. A runaway agent loop
/// is a security and financial issue, not a quality one ([ADR-0008]).
///
/// [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationBudget {
    /// How many times the agent may attempt a repair before Ephemeral stops.
    ///
    /// Seeds the lifecycle's repair budget, which enforces it.
    pub max_repairs: u32,

    /// How long the whole build may take.
    pub max_duration: RetentionPeriod,

    /// How much may be spent with the model provider, in cents.
    ///
    /// `None` means no monetary ceiling, which is only sensible with a local
    /// model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spend_cents: Option<u32>,
}

impl Default for GenerationBudget {
    /// Three repairs, thirty minutes, and five dollars.
    ///
    /// Chosen so that the failure mode of a bad prompt is a wasted half-hour
    /// rather than a surprise on a card.
    fn default() -> Self {
        Self {
            max_repairs: crate::lifecycle::DEFAULT_REPAIR_BUDGET,
            max_duration: RetentionPeriod::seconds(1800),
            max_spend_cents: Some(500),
        }
    }
}

impl GenerationBudget {
    /// Whether this budget is bounded on every axis that can run away.
    ///
    /// A spend ceiling is optional because a local model has no per-token cost,
    /// but iterations and wall clock are never optional.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.max_duration.as_seconds() > 0
    }

    /// A one-line summary.
    #[must_use]
    pub fn describe(&self) -> String {
        let spend = match self.max_spend_cents {
            Some(cents) => format!(", up to ${:.2}", f64::from(cents) / 100.0),
            None => String::new(),
        };
        format!(
            "at most {} repair attempts over {}{spend}",
            self.max_repairs,
            self.max_duration.describe()
        )
    }
}

impl fmt::Display for GenerationBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every default is a real ceiling. A default of "unlimited" would mean the
    /// safe configuration is the one nobody picks.
    #[test]
    fn the_defaults_are_bounded_on_every_axis() {
        let limits = ResourceLimits::default();
        assert!(limits.is_valid());
        assert!(limits.cpu_millis > 0);
        assert!(limits.memory_mib > 0);
        assert!(limits.storage_mib > 0);
        assert!(limits.max_processes > 0, "a fork bomb must be bounded");
        assert!(
            limits.max_runtime.is_some(),
            "a generated app should not be able to run forever by default"
        );

        let budget = GenerationBudget::default();
        assert!(budget.is_valid());
        assert!(budget.max_repairs > 0);
        assert!(
            budget.max_spend_cents.is_some(),
            "the default must not permit unbounded spend"
        );
    }

    #[test]
    fn the_repair_budget_default_matches_the_state_machine() {
        assert_eq!(
            GenerationBudget::default().max_repairs,
            crate::lifecycle::DEFAULT_REPAIR_BUDGET
        );
    }

    /// A zero limit is refused rather than silently meaning "unlimited", which
    /// is the way ceilings get removed by accident.
    #[test]
    fn zero_limits_are_invalid_rather_than_unlimited() {
        for broken in [
            ResourceLimits {
                cpu_millis: 0,
                ..ResourceLimits::default()
            },
            ResourceLimits {
                memory_mib: 0,
                ..ResourceLimits::default()
            },
            ResourceLimits {
                storage_mib: 0,
                ..ResourceLimits::default()
            },
            ResourceLimits {
                max_processes: 0,
                ..ResourceLimits::default()
            },
        ] {
            assert!(!broken.is_valid(), "{broken} should be rejected");
        }
    }

    #[test]
    fn a_service_may_run_without_a_wall_clock_but_keeps_every_other_limit() {
        let service = ResourceLimits::service();
        assert!(service.is_valid());
        assert_eq!(service.max_runtime, None);
        assert_eq!(service.memory_mib, ResourceLimits::default().memory_mib);
    }

    #[test]
    fn limits_describe_themselves_readably() {
        let described = ResourceLimits::default().describe();
        assert!(described.contains("0.50 CPU cores"), "{described}");
        assert!(described.contains("512 MiB of memory"), "{described}");
        assert!(described.contains("stops after 15 minutes"), "{described}");

        let budget = GenerationBudget::default().describe();
        assert!(budget.contains("3 repair attempts"), "{budget}");
        assert!(budget.contains("$5.00"), "{budget}");
    }

    #[test]
    fn limits_round_trip_through_yaml() {
        for limits in [ResourceLimits::default(), ResourceLimits::service()] {
            let yaml = serde_norway::to_string(&limits).unwrap();
            assert_eq!(
                serde_norway::from_str::<ResourceLimits>(&yaml).unwrap(),
                limits
            );
        }

        let budget = GenerationBudget::default();
        let yaml = serde_norway::to_string(&budget).unwrap();
        assert_eq!(
            serde_norway::from_str::<GenerationBudget>(&yaml).unwrap(),
            budget
        );
    }

    #[test]
    fn a_typo_in_a_limits_block_is_an_error() {
        assert!(
            serde_norway::from_str::<ResourceLimits>(
                "cpu_millis: 500\nmemory_mib: 512\nstorage_mib: 1024\nmax_procs: 64\n"
            )
            .is_err(),
            "an unrecognised key must not be silently ignored"
        );
    }
}
