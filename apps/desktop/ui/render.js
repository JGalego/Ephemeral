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

/** What an application has been, and what can be gone back to.
 *
 * Two facts decide what each entry offers, and both come from the service layer
 * rather than being worked out here. `current` is not "the newest entry": the
 * history keeps the version rolled away from, so after one rollback the newest
 * entry and the current version are different things. `source_kept` is not
 * "recorded": a version can be in the history with its source swept away by
 * retention or never kept at all, and only a store knows which — `null` means
 * nobody checked, and is drawn as neither offer nor refusal.
 */
export function versionsSection(versions) {
  const section = element('section', 'versions');
  if (versions.length === 0) return section;

  section.appendChild(
    element(
      'h3',
      'history',
      versions.length === 1 ? '1 version' : `${versions.length} versions`,
    ),
  );

  const list = element('ul', 'version-list');
  for (const version of versions) list.appendChild(versionItem(version));
  section.appendChild(list);

  return section;
}

/** One version, and what may be done with it. */
export function versionItem(version) {
  const item = element('li', `version${version.current ? ' current' : ''}`);
  item.dataset.digest = version.digest;
  item.dataset.sequence = String(version.sequence);
  item.dataset.current = String(Boolean(version.current));

  item.appendChild(element('div', 'what', `Version ${version.sequence} · ${version.digest}`));
  // The reason is written by Ephemeral, not by a model — and goes through
  // `textContent` anyway, because which strings are trusted is not a fact worth
  // relying on twice.
  if (version.reason) item.appendChild(element('div', 'reason', version.reason));

  if (version.current) {
    item.appendChild(element('div', 'now', 'This is the version it is on now.'));
    return item;
  }

  if (version.source_kept !== true) {
    // Deliberately no button for an unknown answer. Offering a rollback that
    // cannot happen wastes somebody's click on a refusal they could have been
    // shown instead.
    item.appendChild(
      element(
        'div',
        'gone',
        version.source_kept === false
          ? 'Its source is not on this machine, so there is nothing to go back to.'
          : 'Whether its source is still on this machine is not known.',
      ),
    );
    return item;
  }

  item.appendChild(rollbackControl(version));
  return item;
}

/** The offer to return to one version. */
export function rollbackControl(version) {
  const controls = element('div', 'revert');
  const button = element('button', 'rollback', 'Return to this version');
  button.dataset.digest = version.digest;
  controls.appendChild(button);

  return controls;
}

/** What a rollback costs, asked before it happens rather than reported after.
 *
 * Rolling back is not undoable by another click: it clears the build and can
 * take permissions away, and the sentence saying so belongs in front of the
 * decision. The same two facts are what the service layer reports afterwards,
 * in the same order, so nobody is told something new once it is too late.
 */
export function rollbackConfirm(version) {
  const controls = element('div', 'revert confirming');

  controls.appendChild(
    element(
      'p',
      'consequence',
      `Return to version ${version.sequence} (${version.digest})? The built image is cleared, ` +
        'so it has to be generated again before it can run — and any permission this version ' +
        'asks for that the current one had stopped needing is taken back.',
    ),
  );

  const confirm = element('button', 'confirm-rollback', 'Return to this version');
  confirm.dataset.digest = version.digest;
  controls.appendChild(confirm);

  const cancel = element('button', 'cancel-rollback', 'Keep the current version');
  controls.appendChild(cancel);

  return controls;
}

/** What a rollback did, in the service layer's own words.
 *
 * Every sentence here arrives from `ephemeral-api`, which is what keeps a
 * rollback in the window from reporting something different from the same
 * rollback in the terminal — including the caution, which is the part somebody
 * has to act on.
 */
export function rollbackNotice(done) {
  const notice = element('div', 'notice-body');
  notice.appendChild(element('p', 'headline', done.headline));
  if (done.caution) notice.appendChild(element('p', 'caution', done.caution));
  notice.appendChild(element('p', 'note', done.note));

  // Pinned to the viewport, like the refusal banner, so it cannot land below
  // the fold — which means it covers whatever is behind it, and a frame of the
  // film showed it sitting squarely on top of the permissions it was telling
  // somebody to go and look at. Three sentences with no way to put them down is
  // a notice that has to be scrolled around.
  notice.appendChild(element('button', 'dismiss', 'Dismiss'));

  return notice;
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
  page.appendChild(versionsSection(detail.versions ?? []));

  return page;
}

/** A failure, said plainly. */
export function problem(message) {
  return element('p', 'problem', message);
}

/** Where somebody says what they want.
 *
 * The front door. Until this existed the window could show applications and
 * answer their questions but not start one, so the first thing anybody had to
 * do with a graphical application was open a terminal.
 *
 * It is a `form` rather than a textarea and a button, because a form is what a
 * keyboard already knows how to submit and what a screen reader already knows
 * how to announce. Nothing here validates the intent: whether a sentence is
 * enough to build from is Ephemeral's judgement, and a window that made that
 * call separately would be making it differently.
 */
export function composer() {
  const form = element('form', 'composer');
  form.id = 'composer';

  const label = element('label', 'ask', 'What do you want?');
  label.setAttribute('for', 'intent');

  const intent = element('textarea', 'intent');
  intent.id = 'intent';
  intent.name = 'intent';
  intent.rows = 2;
  intent.placeholder = 'compare these two CSV files and show me the differences';

  const submit = element('button', 'create', 'Create');
  submit.type = 'submit';

  // Said before somebody asks, not after they have waited. An application
  // starts as a description and nothing else — no code is written and nothing
  // runs until it is generated, which is a separate act. A window that implied
  // otherwise would leave a person watching for a build that was never started.
  const note = element(
    'p',
    'note',
    'This records what you want. Nothing is written or run until you generate it.',
  );

  form.append(label, intent, submit, note);
  return form;
}
