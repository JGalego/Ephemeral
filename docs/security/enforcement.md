# Where each promise is enforced

[`tests/security.rs`](../../crates/ephemeral-core/tests/security.rs) states what
Ephemeral promises and checks it against the domain model. That is necessary and
it is not enough. A rule the model holds and nothing consults is a rule about a
data structure — and that was not hypothetical here: `PermissionLedger::check_app`
carried a doc comment calling itself "the check enforcement points should use"
while every enforcement point used something else, so revoking Ephemeral's own
authority changed a ledger and nothing that runs.

This page is the other half: for every invariant, the code that acts on it, and
the test that exercises it *there*. Phase 3 is finished when this table has no
blanks, which is a claim somebody can check rather than take.

## How to read it

- **Invariant** — the test in `crates/ephemeral-core/tests/security.rs`.
- **Enforced by** — the code that would have to be wrong for the promise to
  break in a running product.
- **Checked at the enforcement point by** — a test against that code, not
  against the model underneath it.

Enforcement tests live in
[`crates/ephemeral-api/tests/enforcement.rs`](../../crates/ephemeral-api/tests/enforcement.rs)
(the service layer both clients call), in the CLI's own modules, in
`crates/ephemeral-runtime` (the container argument vectors), and in
[`apps/desktop/tests/render.test.mjs`](../../apps/desktop/tests/render.test.mjs)
(what a person is actually shown).

## The two permission systems

| Invariant | Enforced by | Checked at the enforcement point by |
|---|---|---|
| `an_application_inherits_none_of_ephemerals_permissions` | `authority::grants` drops every meta-permission before a sandbox is built; the ledger refuses to record one against an application at all | `enforcement::ephemerals_authority_cannot_become_an_applications`, `runtime::a_meta_permission_never_reaches_an_application_sandbox` |
| `ephemeral_inherits_nothing_from_an_application` | `authority::require` asks only about `Principal::Ephemeral` | `authority::a_missing_authority_is_refused_with_the_command_that_grants_it` |
| `the_two_permission_systems_cannot_be_collapsed_into_one` | Two types, two principals, and a ledger that refuses the crossing | `enforcement::ephemerals_authority_cannot_become_an_applications` |
| `one_application_gets_nothing_from_anothers_grants` | `authority::grants` is asked per application, and the sandbox is built from its answer | `enforcement::one_applications_grant_reaches_no_other_application` |
| `one_application_cannot_reach_anothers_storage` | `StorageLayout::app` gives each application its own directory; `/data` is that directory and nothing else is mounted writable | `runtime::a_specification_is_built_from_granted_permissions_only` |
| `a_denial_cannot_be_overridden_by_a_later_grant` | `PermissionLedger::check`, consulted by `authority::grants` on the path to the sandbox | `enforcement::a_denial_survives_and_keeps_the_sandbox_empty` |
| `revocation_stops_what_the_user_asked_to_stop` | `runtime::stop_what_lost_a_permission` stops anything holding a container that the revocation touches; the next sandbox is built without it | `enforcement::revoking_ephemerals_authority_disables_every_application_at_once`, `runtime::a_revoked_grant_no_longer_reaches_the_sandbox` |
| `a_grant_does_not_leak_to_similarly_named_neighbours` | `PathScope` matching in the ledger, and `HostPaths::resolve` deciding what is mounted | `runtime::a_granted_permission_reaches_the_sandbox` (one grant, one mount) |
| `an_egress_allow_list_does_not_cover_lookalike_domains` | `HostScope` matching — and, because Docker cannot filter egress by destination, a scoped grant becomes a refusal rather than the whole internet (`ContainerSpec::refused`, printed when an application starts) | `ephemeral-runtime`'s argument-vector tests; `runtime::report_started` surfaces the refusal |
| `revoking_a_meta_permission_disables_every_application` | `authority::grants` requires both halves, so one revocation empties every sandbox at once | `enforcement::revoking_ephemerals_authority_disables_every_application_at_once` |

## What the generation agent may not do

| Invariant | Enforced by | Checked at the enforcement point by |
|---|---|---|
| `the_agent_cannot_grant_itself_or_anyone_else_a_permission` | `PermissionLedger::allow` refuses a non-human actor; every caller passes the actor that really acted | `enforcement::a_denial_survives_and_keeps_the_sandbox_empty` |
| `the_agent_cannot_revoke_a_permission` | `PermissionLedger::revoke` refuses a non-human actor | as above |
| `the_agent_cannot_sign_off_its_own_output` | `LifecycleEvent::permits`, applied by `generate::step` — the agent reports that it wrote code; the runtime reports that it built and passed | `generate::every_event_is_raised_by_an_actor_entitled_to_raise_it` |
| `the_agent_cannot_delete_or_cancel_an_application` | The same actor authorisation, on `Delete` and `Cancel` | `generate::every_event_is_raised_by_an_actor_entitled_to_raise_it` |
| `the_agent_has_no_available_actions_on_a_ready_application` | The transition table: `Ready` offers the agent nothing | `generate::a_run_starts_from_whichever_event_this_application_can_actually_raise` |

## Manifests, secrets and the record

| Invariant | Enforced by | Checked at the enforcement point by |
|---|---|---|
| `a_hostile_manifest_is_refused_rather_than_partly_applied` | `AppManifest::validate`, called on every save, every load, and every lifecycle transition | `manifest::an_application_cannot_become_ready_with_nothing_to_run` |
| `a_deleted_application_loses_every_permission_at_once` | `commands::delete` calls `revoke_all` in the same operation that deletes | `commands::deleting_withdraws_every_permission_in_the_same_breath` |
| `a_user_can_delete_an_application_in_any_state` | The transition table accepts `Delete` from every state but `Deleted`, and `commands::delete` applies it without asking what state the application is in | `commands::deleting_withdraws_every_permission_in_the_same_breath`, and `ephemeral-core`'s exhaustive transition tests |
| `a_manifest_cannot_carry_a_secret_value` | The manifest holds environment variable *names*; there is no field a value could go in | `ephemeral-core`'s manifest tests |
| `a_secret_cannot_reach_the_audit_record` | `audit::redact` runs on write, not on display | `ephemeral-core`'s redaction tests |
| `secret_use_is_recorded_but_secret_values_are_not` | `AuditEvent::SecretAccessed` records the name; the runtime receives values through `Secrets`, which never reaches an argument vector | `ephemeral-runtime`'s argument-vector tests |
| `altering_a_recorded_decision_is_detected` | `AuditLog::verify`, run by `ephemeral audit` and by `ephemeral doctor` | `doctor`'s workspace checks |
| `a_manifest_cannot_be_used_under_a_different_identity` | `FileAppStore::load` refuses a manifest whose id disagrees with its directory | `storage::file`'s tests |

## Bounds

| Invariant | Enforced by | Checked at the enforcement point by |
|---|---|---|
| `a_new_application_is_bounded_on_every_axis` | `ResourceLimits` in the manifest become `--cpus`, `--memory` and the rest in the container's argument vector | `ephemeral-runtime`'s argument-vector tests |
| `a_zero_limit_is_refused_rather_than_treated_as_unlimited` | `AppManifest::validate` | `ephemeral-core`'s manifest tests |
| `the_repair_loop_cannot_run_forever` | `ephemeral-agent`'s `generate` loop, bounded on attempts, wall clock and spend, with the manifest's own budget passed in | `ephemeral-agent`'s build tests, and the journey test |

## What a person is asked

| Invariant | Enforced by | Checked at the enforcement point by |
|---|---|---|
| `dangerous_requests_are_flagged_for_explicit_confirmation` | `review` requires the word `allow` typed out for a critical permission; the window applies the same rule in `isConsent` rather than trusting which control was clicked | `review`'s tests; `render.test.mjs`'s *a critical permission is not granted by a click* |
| `a_tampered_lifecycle_cannot_resume_into_a_running_state` | `LifecycleState::next` refuses a resume whose recorded state is missing or implausible | `ephemeral-core`'s machine tests |

## What is deliberately not enforced here

**A container escape defeats all of it.** Every row above assumes the sandbox
holds. The [threat model](threat-model.md) says what happens when it does not.

**Disk ceilings are declared and unenforced** for the application's own storage:
Docker's `--storage-opt` needs a backing filesystem Ephemeral cannot assume, so
the number in the manifest is a statement of intent for that axis. It is
recorded as a gap rather than presented as a control.

**A convincing lie in a permission request is not detected by anything.** The
reason an application gives is the model's claim, presented as a claim. Nothing
verifies it, and no enforcement point can.

**On a phone, a decision is recorded and inert.** Both halves of the model reach
the C ABI, and nothing on a device mirrors the operating system's own
permissions into the ledger yet — the platform adapter ADR-0003 describes does
not exist. So allowing an application something on a phone records the decision
and grants no authority, which the page says outright (`"effective": false`)
rather than by omission. It costs nothing today, because a phone generates and
does not run; it is what the adapter has to fix before one does.

**The window cannot grant Ephemeral's *scoped* authority.** It grants and takes back the three
unscoped authorities — a container runtime, the network, a credential — with the
word `allow` typed out, and shows everything else Ephemeral holds with a control
to take it back. What it will not do is compose a path: granting
`read:~/Downloads/**` means choosing a region of the filesystem, and a window
that built one out of a text field would be a window that can grant Ephemeral
something nobody typed. Those are granted from the terminal, where the path is
the argument.
