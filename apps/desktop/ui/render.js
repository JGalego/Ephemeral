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
  // A capability the person allowed and Ephemeral itself may not carry out is
  // held and does nothing. It is neither hidden — the decision is theirs and
  // still stands — nor drawn as working, which would put authority on screen
  // that the sandbox does not give.
  const inert = held && permission.effective === false;
  const item = element(
    'li',
    `permission risk-${permission.risk}${held ? ' held' : ''}${inert ? ' inert' : ''}`,
  );
  item.dataset.capability = permission.capability;
  item.dataset.risk = permission.risk;
  item.dataset.effective = String(permission.effective !== false);

  item.appendChild(element('div', 'wants', permission.wants));

  if (inert) {
    item.appendChild(
      element(
        'div',
        'blocked',
        permission.blocked_by
          ? `This does nothing right now: Ephemeral itself has not been allowed to ${permission.blocked_by}.`
          : 'This does nothing right now.',
      ),
    );
  }

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

  // Isolated is about what it can reach, not about what it was allowed: an
  // application every one of whose grants is inert reaches nothing, and saying
  // so is true. What it holds is listed underneath either way, so "it can see
  // nothing of yours" is never the whole story where there is more to tell.
  if (permissions.isolated) {
    section.appendChild(
      element('p', 'isolated', 'This app can see nothing of yours: no files, no network.'),
    );
  }

  if (permissions.allowed.length > 0) {
    const dormant = permissions.allowed.every((permission) => permission.effective === false);
    const count =
      permissions.allowed.length === 1
        ? '1 thing you have allowed'
        : `${permissions.allowed.length} things you have allowed`;

    // Labelled, because an unlabelled list of capabilities next to another
    // unlabelled list of capabilities tells a person nothing about which is
    // which.
    section.appendChild(
      element('h3', 'holds', dormant ? `${count}, which do nothing yet` : count),
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

/** What a page an application wrote is allowed to be, while it is shown.
 *
 * Prepended to the application's own markup, so it is the first policy the
 * parser meets and therefore the one that applies. `none` for everything, with
 * two exceptions that cost nothing: inline styles, so a page can look like
 * something, and `data:` images, so it can draw a chart it computed itself.
 * Nothing may be fetched from anywhere.
 */
const PAGE_POLICY =
  '<meta http-equiv="Content-Security-Policy" ' +
  "content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'\">";

/** The page an application wrote, in a frame that can do nothing else.
 *
 * **The interesting part is everything this cannot do.** A WebAssembly
 * application has no socket, so it writes a page rather than serving one — and
 * the page is then shown in a frame with an empty `sandbox`, which withholds
 * scripts, same-origin access, forms and navigation, plus a policy that permits
 * no subresource loads at all.
 *
 * Both, not either. `sandbox` stops the page running code; the policy stops it
 * putting what it was shown into the query string of an image. Markup a model
 * wrote is untrusted content and is treated as content.
 *
 * That is also why showing somebody a user interface costs no network
 * permission. An application serving a page on a port needs one, and the same
 * permission then lets it talk to anybody.
 */
export function pageFrame(html) {
  const frame = document.createElement('iframe');
  frame.className = 'page';
  // Empty, not omitted: an absent `sandbox` attribute is no sandbox, and the
  // two are one character apart.
  frame.setAttribute('sandbox', '');
  frame.setAttribute('srcdoc', PAGE_POLICY + html);
  return frame;
}

/** What starting an application said, as a notice.
 *
 * Two shapes, because starting has two. A container is now running and the
 * useful thing to say is what it can reach. A WebAssembly module has already
 * finished, and the useful thing to say is what it produced — so the output is
 * the notice rather than a line pointing at somewhere else to look.
 */
export function runNotice(run) {
  const notice = element('div', 'notice-body');

  if (run.finished) {
    notice.appendChild(
      element(
        'p',
        'headline',
        run.finished.succeeded ? 'It ran.' : `It failed (${run.finished.exit_code}).`,
      ),
    );
  } else {
    notice.appendChild(element('p', 'headline', `Started. It is ${run.state.toLowerCase()}.`));
  }

  // Before the output, and in Ephemeral's own voice rather than inside content
  // the application wrote — where it could be styled to look like anything.
  if (run.inert) notice.appendChild(element('p', 'caution', run.inert));
  for (const refusal of run.refused ?? []) notice.appendChild(element('p', 'caution', refusal));

  if (run.finished) {
    if (run.finished.shown === 'page') {
      notice.appendChild(pageFrame(run.finished.output));
    } else if (run.finished.output.trim() === '') {
      notice.appendChild(element('p', 'note', 'It ran and printed nothing.'));
    } else {
      // `textContent`, through `element`, so output is text whatever it
      // contains. The only place markup from an application is ever parsed is
      // the frame above, and only when it said it was writing a page.
      notice.appendChild(element('pre', 'output', run.finished.output));
    }
  }

  for (const said of run.confinement ?? []) notice.appendChild(element('p', 'note', said));

  notice.appendChild(element('button', 'dismiss', 'Dismiss'));

  return notice;
}

/// What can be done to this application right now, as buttons.
///
/// Driven by the events the lifecycle says a person may raise, not by this
/// window's reading of a few booleans. Those two are not the same thing: the
/// window used to work out its own buttons, and offered Stop for an application
/// that had only been built. The state machine already answers this exactly
/// (`available_events`), the service layer carries the answer as `can`, and
/// every client draws the same set.
///
/// Anything it cannot do is absent rather than disabled — a row of greyed-out
/// buttons is a puzzle, and the state is already on the page in words.
export function actions(detail) {
  const row = element('div', 'actions');
  const state = detail.summary.state_kind;
  const built = detail.runtime !== null && detail.runtime !== undefined;
  const can = new Set(detail.can ?? []);

  const add = (className, label, dataset = {}) => {
    const button = element('button', className, label);
    for (const [key, value] of Object.entries(dataset)) button.dataset[key] = value;
    row.appendChild(button);
    return button;
  };

  if (can.has('start')) {
    add('start', 'Run');
  }
  if (can.has('stop')) {
    add('halt', 'Stop');
  }
  if (can.has('pause')) {
    add('hold', 'Pause', { event: 'pause' });
  }
  if (can.has('resume')) {
    add('hold', 'Resume', { event: 'resume' });
  }

  // Generating is offered when there is nothing built yet, and when the last
  // attempt ended somewhere it can be picked up from. A ready application is
  // running code somebody approved: replacing it is not a button.
  if (!built || ['attention', 'failed'].includes(state)) {
    // The provider is chosen next to the button that uses it, and `mock` is
    // first because it is the one that works with nothing installed and no
    // account anywhere. Which providers exist is the engine's answer, not this
    // window's.
    const picker = element('select', 'provider');
    for (const name of detail.providers ?? ['mock']) {
      const option = element('option', null, name);
      option.value = name;
      picker.appendChild(option);
    }
    picker.setAttribute('aria-label', 'Which model to build it with');
    row.appendChild(picker);

    add('generate', built ? 'Generate again' : 'Generate');
  }

  if (can.has('restore')) {
    add('move', 'Restore', { event: 'restore' });
  }
  if (can.has('archive')) {
    add('move', 'Archive', { event: 'archive' });
  }

  // Purging is not a lifecycle event: a deleted application has nowhere left to
  // go, and purging removes the record rather than moving it.
  if (can.has('delete')) {
    add('move danger', 'Delete', { event: 'delete' });
  } else if (state === 'deleted') {
    add('purge danger', 'Purge for good');
  }

  return row;
}

/// Where somebody says how to run an application, and what to give it.
///
/// The arguments are the application's own. The placeholder is built from a
/// mount it actually has, because the paths it answers to are the ones inside
/// its sandbox and nothing else on this page would say so.
export function runPanel(detail) {
  const form = element('form', 'run');
  form.dataset.id = detail.summary.id;

  const label = element('label', 'ask', 'Anything to pass it?');
  label.setAttribute('for', 'arguments');

  const field = element('input', 'arguments');
  field.type = 'text';
  field.id = 'arguments';
  field.placeholder = '/data/left.csv /data/right.csv';
  field.setAttribute('aria-label', 'Arguments for the application');

  const note = element(
    'p',
    'note',
    'Paths here are the ones the application sees: a folder you allowed appears under /mnt, ' +
      'and its own storage is /data.',
  );

  form.append(label, field, note);
  return form;
}

/// A generation run, while it is happening and once it is not.
///
/// Progress is the application's own lifecycle — planning, writing, building,
/// testing — because that is what is really true and it is already recorded as
/// it happens. A window that invented a progress bar would be drawing a number
/// nothing produced.
export function generationPanel(state, explanation) {
  const panel = element('div', 'generation');

  if (state?.running) {
    panel.classList.add('running');
    panel.appendChild(element('p', 'what', explanation ?? 'Working…'));
    panel.appendChild(
      element(
        'p',
        'note',
        'This takes minutes. You can leave this page; it keeps going, and the application ' +
          'says where it got to.',
      ),
    );
    return panel;
  }

  if (state?.failed) {
    panel.classList.add('failed');
    panel.appendChild(element('p', 'what', state.failed));
    panel.appendChild(dismissal());
    return panel;
  }

  if (!state?.built) return panel;

  const built = state.built;
  panel.classList.add('built');
  panel.appendChild(element('p', 'what', built.headline));
  panel.appendChild(element('p', 'note', built.how_it_went));
  if (built.version) panel.appendChild(element('p', 'note', `version ${built.version}`));

  for (const warning of built.warnings ?? []) {
    panel.appendChild(element('p', 'caution', warning));
  }

  if (built.requests.length > 0) {
    panel.appendChild(
      element(
        'p',
        'note',
        built.requests.length === 1
          ? 'It will ask for 1 thing. It holds none of it yet.'
          : `It will ask for ${built.requests.length} things. It holds none of them yet.`,
      ),
    );
  }

  if (built.widened) {
    panel.appendChild(element('p', 'caution', built.widened));
    if (built.grants_withdrawn > 0) {
      panel.appendChild(
        element(
          'p',
          'caution',
          `${built.grants_withdrawn} permission(s) you had allowed were withdrawn, because they ` +
            'no longer cover what it now asks for.',
        ),
      );
    }
  } else if (built.unchanged) {
    panel.appendChild(element('p', 'note', built.unchanged));
  }

  panel.appendChild(dismissal());
  return panel;
}

/// The control that puts a finished run's report away.
function dismissal() {
  const controls = element('div', 'decide');
  controls.appendChild(element('button', 'acknowledge', 'Dismiss'));
  return controls;
}

/// What an application has been, and what it last printed.
///
/// The output is shown as it came out — `textContent` into a `pre`, never
/// markup — because it is written by generated code and is exactly the sort of
/// thing that would carry an injection if anything here interpreted it.
export function logsSection(logs) {
  const section = element('section', 'logs');

  section.appendChild(element('h3', 'history', 'What has happened'));
  const list = element('ul', 'history');
  for (const entry of logs.history ?? []) {
    const item = element('li', 'moment');
    item.appendChild(element('div', 'state', entry.state));
    item.appendChild(element('div', 'what', entry.what));
    if (entry.error) item.appendChild(element('div', 'no-reason', entry.error));
    list.appendChild(item);
  }
  if ((logs.history ?? []).length === 0) {
    list.appendChild(element('li', 'empty', 'Nothing yet.'));
  }
  section.appendChild(list);

  section.appendChild(element('h3', 'holds', 'What it printed'));
  if (logs.output === null || logs.output === undefined) {
    section.appendChild(
      element(
        'p',
        'empty',
        'Nothing to show: there is no container to ask. Its output is kept only while one exists.',
      ),
    );
  } else if (logs.output.trim() === '') {
    section.appendChild(element('p', 'empty', 'It has not printed anything.'));
  } else {
    section.appendChild(element('pre', 'output', logs.output));
  }

  return section;
}

/// What Ephemeral itself may do.
///
/// Its own section, and never folded in with an application's: they are two
/// permission systems, and showing them as one list is the confusion the whole
/// model exists to prevent. This is the answer to "why did nothing happen",
/// which on a new installation is usually a permission Ephemeral was never
/// given rather than anything broken.
export function authoritySection(items) {
  const section = element('section', 'authority');
  section.appendChild(element('h2', 'name', 'What Ephemeral itself may do'));
  section.appendChild(
    element(
      'p',
      'note',
      'Separate from what any application may do, and never inherited in either direction. ' +
        'An application can only do something Ephemeral is also allowed to carry out.',
    ),
  );

  const list = element('ul', 'authorities');
  for (const item of items) list.appendChild(authorityItem(item));
  section.appendChild(list);

  return section;
}

/// One thing Ephemeral may or may not do.
export function authorityItem(item) {
  const entry = element('li', `authority risk-${item.risk}${item.granted ? ' held' : ''}`);
  entry.dataset.capability = item.capability;
  entry.dataset.granted = String(item.granted);

  entry.appendChild(element('div', 'wants', item.wants));
  entry.appendChild(element('div', 'if-allowed', item.if_allowed));

  const controls = element('div', 'decide');

  if (item.granted) {
    const revoke = element('button', 'revoke-authority', 'Take this back');
    revoke.dataset.capability = item.capability;
    controls.appendChild(revoke);

    // Said next to the control that would lose it: a scoped authority is one
    // this window cannot hand back, because it cannot choose a path. Somebody
    // about to take it back should know that before they do.
    if (!item.grantable) {
      controls.appendChild(
        element(
          'div',
          'note',
          'Granting this again means naming the path, which is done from the terminal: ' +
            `ephemeral grant ephemeral ${item.capability}`,
        ),
      );
    }
  } else if (item.grantable) {
    // Typed, always. This authority outlives every application and covers all
    // of them at once, so asking for it as casually as for one folder would
    // teach the wrong reflex.
    const field = element('input', 'confirm');
    field.type = 'text';
    field.placeholder = 'type allow';
    field.setAttribute('aria-label', `Type allow to let Ephemeral ${item.wants}`);
    controls.appendChild(field);

    const confirm = element('button', 'grant-authority', 'Confirm');
    confirm.dataset.capability = item.capability;
    controls.appendChild(confirm);
  } else {
    controls.appendChild(
      element(
        'div',
        'note',
        'Granted from the terminal, where the path is written out: ' +
          `ephemeral grant ephemeral ${item.capability}`,
      ),
    );
  }

  entry.appendChild(controls);
  return entry;
}

/// What this machine can and cannot do, with the remedy for anything it cannot.
export function diagnosticsSection(checks) {
  const section = element('section', 'diagnostics');
  section.appendChild(element('h2', 'name', 'This machine'));

  const list = element('ul', 'checks');
  for (const check of checks) {
    const item = element(
      'li',
      `check ${check.ok === true ? 'ok' : check.ok === false ? 'bad' : 'note'}`,
    );
    item.appendChild(element('div', 'what', check.what));
    if (check.advice) item.appendChild(element('div', 'advice', check.advice));
    list.appendChild(item);
  }
  section.appendChild(list);

  return section;
}

/// The security record, newest first.
export function activitySection(entries) {
  const section = element('section', 'activity');
  section.appendChild(element('h2', 'name', 'Security record'));

  const list = element('ul', 'entries');
  for (const entry of entries) {
    const item = element('li', 'entry');
    item.appendChild(element('div', 'what', entry.summary));
    item.appendChild(element('div', 'who', entry.actor));
    list.appendChild(item);
  }
  if (entries.length === 0) list.appendChild(element('li', 'empty', 'Nothing recorded yet.'));
  section.appendChild(list);

  return section;
}

/** An application's page. */
export function applicationDetail(detail) {
  const page = element('section', 'detail');
  page.dataset.id = detail.summary.id;

  const back = element('button', 'back', '← All applications');
  page.appendChild(back);

  // The state travels with the name, drawn as the same pill the list uses, and
  // the kind goes on the page so the colour is the lifecycle's opinion rather
  // than this window's. Without it the only account of where an application is
  // was a sentence of prose, and the two clients disagreed about how loudly to
  // say "Running".
  page.dataset.state = detail.summary.state_kind;
  const heading = element('h2', 'name', detail.summary.name);
  heading.appendChild(element('span', 'state', detail.summary.state));
  page.appendChild(heading);

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
  page.appendChild(actions(detail));
  if ((detail.can ?? []).includes('start')) page.appendChild(runPanel(detail));
  page.appendChild(element('div', 'generation-slot'));
  page.appendChild(permissionsSection(detail.permissions));
  page.appendChild(versionsSection(detail.versions ?? []));
  page.appendChild(element('div', 'logs-slot'));

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
