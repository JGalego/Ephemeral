// What the desktop window shows, as pure functions from a view to DOM.
//
// Separated from everything that talks to Tauri so it can be tested in an
// ordinary browser. A desktop UI that could only be exercised by opening a
// window would be a UI nobody ever tested, which is how a permission prompt
// ends up saying something different from the one in the terminal.
//
// These functions never invent text. Every phrase a person reads comes from
// `ephemeral-api`, so the same promise is worded the same way in both clients.

/** Builds an element, escaping text by construction rather than by discipline. */
export function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  // `textContent`, never `innerHTML`. Application names and the reasons a model
  // gives are untrusted strings; there is no route from either into markup.
  if (text !== undefined && text !== null) node.textContent = text;
  return node;
}

/** One application, as a list item. */
export function applicationItem(summary) {
  const item = element('li', 'application');
  item.dataset.id = summary.id;
  item.dataset.state = summary.state_kind;

  const name = element('div', 'name', summary.name);

  // The only thing on the list that shouts, because an application waiting on
  // a decision is what a person most needs to notice.
  if (summary.awaiting_decision > 0) {
    const badge = element(
      'span',
      'awaiting',
      summary.awaiting_decision === 1
        ? '1 decision waiting'
        : `${summary.awaiting_decision} decisions waiting`,
    );
    name.appendChild(badge);
  }

  item.appendChild(name);
  if (summary.purpose) item.appendChild(element('div', 'purpose', summary.purpose));
  item.appendChild(element('div', 'state', summary.state));

  // What an application already holds, which the list said nothing about until
  // somebody looked at a recording of it. An app that had been allowed to reach
  // the whole internet was drawn exactly like one that can see nothing of
  // yours: same words, same colour, no difference at all. The count alone does
  // not carry that, so the risk comes with it.
  if (summary.granted > 0) {
    // Deliberately no fallback risk. Defaulting an unknown one to `low` paints
    // a reassuring green on an application that might hold the widest
    // permission Ephemeral offers — the one case where guessing is worst. An
    // unknown risk is drawn as unknown, in the ordinary text colour, and says
    // nothing it cannot support.
    const risk = summary.highest_granted_risk;
    const holds = element(
      'div',
      risk ? `grants risk-${risk}` : 'grants',
      summary.granted === 1 ? 'Allowed 1 thing' : `Allowed ${summary.granted} things`,
    );
    holds.dataset.highestRisk = risk ?? '';
    item.appendChild(holds);
  }

  return item;
}

/** The whole list, or the sentence that replaces it. */
export function applicationList(summaries, { showPutAway = false } = {}) {
  const visible = summaries.filter((summary) => showPutAway || !summary.put_away);
  const list = element('ul');

  for (const summary of visible) list.appendChild(applicationItem(summary));

  // "No applications yet" would be a lie when some are merely archived, and a
  // user would think theirs had vanished.
  const hidden = summaries.length - visible.length;
  if (visible.length === 0) {
    list.appendChild(
      element(
        'li',
        'empty',
        hidden > 0
          ? `Nothing active. ${hidden} archived or deleted.`
          : 'No applications yet. Ask for one and Ephemeral will build it.',
      ),
    );
  }

  return list;
}

/** One permission, phrased as a question a person can answer.
 *
 * `held` means the person already said yes. Looking at the first film of this
 * window showed a granted permission offering "Allow" again, in the same colour
 * as an unanswered one — so nothing on screen distinguished what somebody had
 * agreed to from what they were being asked. No test caught that, because every
 * test asserted text and the problem was that two different things looked the
 * same.
 */
export function permissionItem(permission, { held = false } = {}) {
  const item = element('li', `permission risk-${permission.risk}${held ? ' held' : ''}`);
  item.dataset.capability = permission.capability;
  item.dataset.risk = permission.risk;

  item.appendChild(element('div', 'wants', permission.wants));

  // The reason is the only part of a request a person cannot check, so a
  // missing one says so rather than being quietly omitted — an absent reason
  // must look different from a good one.
  if (permission.reason) {
    item.appendChild(element('div', 'reason', `It says: ${permission.reason}`));
  } else {
    item.appendChild(element('div', 'no-reason', 'It gives no reason for wanting this.'));
  }

  item.appendChild(element('div', 'if-allowed', permission.if_allowed));
  item.appendChild(
    element(
      'div',
      'revocable',
      permission.revocable
        ? 'You can take this back at any time.'
        : 'This cannot be taken back.',
    ),
  );

  item.dataset.needsConfirmation = String(permission.needs_explicit_confirmation);
  item.dataset.held = String(held);
  item.appendChild(held ? revokeControl() : decisionControls(permission));

  return item;
}

/** The control for taking back something already allowed. */
export function revokeControl() {
  const controls = element('div', 'decide');
  const revoke = element('button', 'revoke', 'Take this back');
  revoke.dataset.decision = 'revoke';
  controls.appendChild(revoke);

  return controls;
}

/// The controls for answering one request.
///
/// A high-risk permission must not be accepted by the same reflex as a low-risk
/// one. A plain click allows an ordinary request; a critical one requires
/// typing the word `allow`, so a habit formed on easy questions does not carry
/// over to the one that matters. Refusing is always one click, because making
/// "no" harder than "yes" is how consent gets manufactured.
export function decisionControls(permission) {
  const controls = element('div', 'decide');

  if (permission.needs_explicit_confirmation) {
    const field = element('input', 'confirm');
    field.type = 'text';
    field.placeholder = 'type allow';
    field.setAttribute('aria-label', `Type allow to permit: ${permission.wants}`);
    controls.appendChild(field);

    // A field with nothing to submit it is a dead end. The first recording of
    // this window had exactly that: somebody could type `allow` and nothing
    // would happen, because the affirmative button had been removed and
    // nothing replaced it. The button is deliberately not called "Allow" —
    // it confirms what was typed, and typing the wrong thing still refuses.
    const confirm = element('button', 'confirm-allow', 'Confirm');
    confirm.dataset.decision = 'allow';
    controls.appendChild(confirm);
  } else {
    const allow = element('button', 'allow', 'Allow');
    allow.dataset.decision = 'allow';
    controls.appendChild(allow);
  }

  const deny = element('button', 'deny', 'Deny');
  deny.dataset.decision = 'deny';
  controls.appendChild(deny);

  return controls;
}

/// Whether an answer to a request should be treated as consent.
///
/// The same rule the terminal holds, in the same words, because it is the same
/// promise: nothing is granted without an answer, and a critical permission
/// takes the word rather than a keystroke.
export function isConsent(permission, answer) {
  if (!permission.needs_explicit_confirmation) return answer === 'allow';

  return typeof answer === 'string' && answer.trim().toLowerCase() === 'allow';
}

/** What an application is allowed to do, and what it is still asking for. */
export function permissionsSection(permissions) {
  const section = element('section', 'permissions');

  if (permissions.isolated) {
    section.appendChild(
      element('p', 'isolated', 'This app can see nothing of yours: no files, no network.'),
    );
  } else {
    // Labelled, because an unlabelled list of capabilities next to another
    // unlabelled list of capabilities tells a person nothing about which is
    // which.
    section.appendChild(
      element(
        'h3',
        'holds',
        permissions.allowed.length === 1
          ? '1 thing you have allowed'
          : `${permissions.allowed.length} things you have allowed`,
      ),
    );

    const allowed = element('ul', 'allowed');
    for (const permission of permissions.allowed) {
      allowed.appendChild(permissionItem(permission, { held: true }));
    }
    section.appendChild(allowed);
  }

  if (permissions.outstanding.length > 0) {
    section.appendChild(
      element(
        'h3',
        'waiting',
        permissions.outstanding.length === 1
          ? '1 thing it is asking for'
          : `${permissions.outstanding.length} things it is asking for`,
      ),
    );
    const outstanding = element('ul', 'outstanding');
    for (const permission of permissions.outstanding) {
      outstanding.appendChild(permissionItem(permission));
    }
    section.appendChild(outstanding);
  }

  return section;
}

/** An application's page. */
export function applicationDetail(detail) {
  const page = element('section', 'detail');
  page.dataset.id = detail.summary.id;

  const back = element('button', 'back', '← All applications');
  page.appendChild(back);

  page.appendChild(element('h2', 'name', detail.summary.name));
  page.appendChild(element('p', 'explanation', detail.explanation));

  if (detail.runtime) {
    // Whether data leaves the device is the fact a person most needs and is
    // least likely to read, so it is its own line rather than buried in prose.
    page.appendChild(
      element(
        'p',
        detail.runtime.runs_locally ? 'local' : 'remote',
        detail.runtime.isolation,
      ),
    );
  }

  page.appendChild(element('p', 'limits', detail.limits.description));
  page.appendChild(permissionsSection(detail.permissions));

  return page;
}

/** A failure, said plainly. */
export function problem(message) {
  return element('p', 'problem', message);
}
