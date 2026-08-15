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

/** One permission, phrased as a question a person can answer. */
export function permissionItem(permission) {
  const item = element('li', `permission risk-${permission.risk}`);
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

  // A high-risk permission must not be accepted by the same reflex as a low-risk
  // one, so the affirmative control is deliberately not a plain button here.
  item.dataset.needsConfirmation = String(permission.needs_explicit_confirmation);

  return item;
}

/** What an application is allowed to do, and what it is still asking for. */
export function permissionsSection(permissions) {
  const section = element('section', 'permissions');

  if (permissions.isolated) {
    section.appendChild(
      element('p', 'isolated', 'This app can see nothing of yours: no files, no network.'),
    );
  } else {
    const allowed = element('ul', 'allowed');
    for (const permission of permissions.allowed) allowed.appendChild(permissionItem(permission));
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
