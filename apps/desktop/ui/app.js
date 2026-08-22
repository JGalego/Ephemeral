// The desktop window.
//
// Everything that decides what to show lives in `render.js`, which is pure and
// tested. This file does one thing: ask Ephemeral for a view and hand it over.
// It decides nothing — a client that evaluated a permission or computed a
// transition would be a second, subtly different Ephemeral.

import {
  activitySection,
  applicationList,
  applicationDetail,
  authoritySection,
  composer,
  diagnosticsSection,
  generationPanel,
  isConsent,
  logsSection,
  problem,
  rollbackConfirm,
  rollbackNotice,
} from './render.js';

/** The pending re-read of a page whose application is being generated. */
let watching = null;

/** Calls a command, or explains why it could not. */
async function ask(command, args = {}) {
  // Tauri injects this. Absent it, the window is being opened in a browser for
  // testing, and saying so beats a stack trace.
  const invoke = window.__TAURI__?.core?.invoke;
  if (!invoke) throw new Error('This window is not running inside Ephemeral.');

  return invoke(command, args);
}

function show(node) {
  const list = document.getElementById('list');
  list.replaceWith(node);
  node.id = 'list';
}

function reportProblem(message) {
  const banner = document.getElementById('problem');
  banner.textContent = message;
  banner.hidden = false;
}

/** Says what just happened, in the service layer's words.
 *
 * Outside the page rather than inside it, because what a rollback has to say —
 * that permissions were taken back, that there is nothing built any more —
 * outlives the re-render that follows it. Rendered into the page, it would be
 * replaced by the page it is describing.
 */
function reportOutcome(node) {
  const banner = document.getElementById('notice');
  banner.replaceChildren(node);
  banner.hidden = false;
  // To the top, because the banner is pinned to the viewport and the page
  // underneath is wherever somebody had scrolled to. A film of this showed the
  // notice printed across the middle of the permissions it was telling somebody
  // to go and check.
  window.scrollTo({ top: 0 });
}

function clearOutcome() {
  const banner = document.getElementById('notice');
  banner.replaceChildren();
  banner.hidden = true;
}

/** Re-reads the list without deciding which page is on screen.
 *
 * Kept separate from `refresh` because a decision has to update the list — the
 * badge counting what is still waiting — while leaving the person where they
 * were. Reloading and navigating were once the same function, and the result
 * was that answering a permission threw you back to the list, so the page
 * confirming what you had just allowed was never seen. Filming the window is
 * what showed it: the recording simply walked out of the page mid-sentence.
 */
async function reload() {
  // Before drawing anything: an application that crashed while nobody was
  // looking still reads as running until something asks, and this is what asks.
  // Quiet on failure — a machine with no container runtime has nothing to
  // reconcile against, and saying so on every redraw would be noise.
  await ask('sweep').catch(() => []);

  const summaries = await ask('applications');
  show(applicationList(summaries));
  document.getElementById('problem').hidden = true;
}

/** Goes to the list. */
async function refresh() {
  try {
    clearOutcome();
    await reload();
    await showMachine();
    document.getElementById('applications').hidden = false;
    document.getElementById('compose').hidden = false;
    document.getElementById('detail').hidden = true;
  } catch (error) {
    // The message from the core is already written for a person; adding to it
    // would be inventing detail this layer does not have.
    reportProblem(String(error.message ?? error));
  }
}

/** Goes to one application's page. */
async function open(id) {
  try {
    await ask('refresh', { id }).catch(() => null);

    const detail = await ask('application', { id });
    detail.providers = await ask('providers').catch(() => ['mock']);
    const panel = document.getElementById('detail');
    panel.replaceChildren(applicationDetail(detail));
    panel.hidden = false;
    await Promise.all([showGeneration(id, detail), showLogs(id)]);
    // Replace the list rather than stacking beneath it. The first recording of
    // this window showed both at once, which reads as two pages at the same
    // time.
    document.getElementById('applications').hidden = true;
    document.getElementById('machine').hidden = true;
    // The composer goes with it. Leaving "what do you want?" above the page
    // somebody is reading offers a new application instead of the one in front
    // of them.
    document.getElementById('compose').hidden = true;
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

/** Records one decision, then reloads so the page shows what is now true. */
async function decide(item, answer) {
  const page = item.closest('.detail');
  const permission = {
    needs_explicit_confirmation: item.dataset.needsConfirmation === 'true',
  };

  // Consent is judged by the same rule the terminal uses, and judged *here*
  // rather than by trusting which control was clicked. A window that inferred
  // consent from a button would grant on a stray click into a text field.
  if (answer !== 'deny' && !isConsent(permission, answer)) {
    reportProblem('Type `allow` to permit this. Nothing has been decided.');
    return;
  }

  try {
    await ask('decide', {
      id: page.dataset.id,
      capability: item.dataset.capability,
      target: null,
      allow: answer !== 'deny',
    });
    // Stay on the page and re-render it, so what was just decided is visible as
    // decided. The list is re-read too, because its badge is now wrong, but
    // re-reading it must not navigate away from what somebody is reading.
    await open(page.dataset.id);
    await reload();
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

/** Returns an application to a version it used to be.
 *
 * The whole operation is `ephemeral-api`'s — the source going back, the image
 * being cleared, the grants the older version must not inherit being withdrawn
 * — so the window rolls back the way the terminal does rather than in a fourth
 * similar order. What is left here is asking, and then showing what it said.
 */
async function rollback(id, digest) {
  try {
    const done = await ask('rollback', { id, version: digest });
    // The page first, so what is on screen is what is now true, and the notice
    // second, so it survives the re-render rather than being replaced by it.
    await open(id);
    await reload();
    reportOutcome(rollbackNotice(done));
    document.getElementById('problem').hidden = true;
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}


/** Draws whatever a generation run is doing, if one is. */
async function showGeneration(id, detail) {
  const slot = document.querySelector('.generation-slot');
  if (!slot) return;

  const state = await ask('generation', { id }).catch(() => null);
  slot.replaceChildren(generationPanel(state, detail?.explanation));

  // Polled while it runs, because generation takes minutes and the progress
  // that matters — planning, writing, building, testing — is the application's
  // own lifecycle, which is saved as it happens.
  if (state?.running) {
    clearTimeout(watching);
    watching = setTimeout(() => {
      const page = document.querySelector('.detail');
      if (page?.dataset.id === id) open(id);
    }, 2000);
  }
}

/** What an application has been, and what it printed. */
async function showLogs(id) {
  const slot = document.querySelector('.logs-slot');
  if (!slot) return;

  const logs = await ask('logs', { id, lines: 50 }).catch(() => null);
  if (logs) slot.replaceChildren(logsSection(logs));
}

/** What Ephemeral itself may do, and what this machine can do. */
async function showMachine() {
  const panel = document.getElementById('machine');
  if (!panel) return;

  try {
    const [authority, checks, activity] = await Promise.all([
      ask('authority'),
      ask('diagnostics'),
      ask('activity', { limit: 12 }),
    ]);

    panel.replaceChildren(
      authoritySection(authority),
      diagnosticsSection(checks),
      activitySection(activity),
    );
    panel.hidden = false;
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

/** Starts an application, and says what it can reach. */
async function start(id, argumentLine) {
  try {
    // Split the way a shell would, and no further: what the arguments mean is
    // the application's business. A window that interpreted them would invent
    // behaviour the terminal does not have.
    const args = argumentLine.trim() === '' ? [] : argumentLine.trim().split(/\s+/);
    const run = await ask('start', { id, arguments: args });

    await open(id);
    await reload();
    reportOutcome(runNotice(run));
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

/** What starting an application said, as a notice. */
function runNotice(run) {
  const notice = document.createElement('div');
  notice.className = 'notice-body';

  const line = (className, text) => {
    const node = document.createElement('p');
    node.className = className;
    node.textContent = text;
    notice.appendChild(node);
  };

  line('headline', `Started. It is ${run.state.toLowerCase()}.`);
  if (run.inert) line('caution', run.inert);
  for (const refusal of run.refused) line('caution', refusal);
  for (const said of run.confinement) line('note', said);

  const dismiss = document.createElement('button');
  dismiss.className = 'dismiss';
  dismiss.textContent = 'Dismiss';
  notice.appendChild(dismiss);

  return notice;
}

/** Records what somebody asked for, then shows them the result.
 *
 * The intent is sent exactly as typed. Trimming, emptiness and naming are all
 * decided by `ephemeral-api`, so the window cannot accept something the
 * terminal would refuse — or refuse something it would accept.
 */
async function submitIntent(form) {
  const field = form.querySelector('textarea.intent');
  const button = form.querySelector('button.create');

  // Creating touches the disk, and a second click before the first returns
  // would create a second application nobody asked for.
  button.disabled = true;
  try {
    const created = await ask('create', { intent: field.value });
    field.value = '';
    // Straight to the new application's page: it says what state it is in and
    // what happens next, in the lifecycle's own words rather than the window's.
    await reload();
    await open(created.id);
  } catch (error) {
    reportProblem(String(error.message ?? error));
  } finally {
    button.disabled = false;
  }
}


/** Records a decision about something Ephemeral itself may do. */
async function decideAuthority(capability, allow) {
  try {
    await ask('decide_authority', { capability, allow });
    await showMachine();
    await reload();
    document.getElementById('problem').hidden = true;
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

/** Runs a command that needs no answer, then re-reads the page. */
async function simply(command, args, id) {
  try {
    await ask(command, args);
    await open(id);
    await reload();
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

/** Puts an application away, brings it back, or throws it away. */
async function moveApp(id, event) {
  try {
    const moved = await ask('move_app', { id, event });
    await reload();
    await open(id);
    reportOutcome(movedNotice(moved));
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

/** Destroying an application is asked twice, and the second time says so. */
function confirmDestruction(button, id) {
  const controls = button.closest('.actions');
  const warning = document.createElement('p');
  warning.className = 'consequence';
  warning.textContent =
    'Purging destroys everything it holds — source, data, logs, artifacts. There is no way back.';

  const confirm = document.createElement('button');
  confirm.className = 'confirm-purge danger';
  confirm.dataset.id = id;
  confirm.textContent = 'Purge for good';

  button.replaceWith(warning, confirm);
}

/** Destroys an application and everything it holds. */
async function purge(id) {
  try {
    const gone = await ask('purge', { id });
    await refresh();
    reportOutcome(movedNotice(gone));
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

/** What a move said, as a notice. */
function movedNotice(moved) {
  const notice = document.createElement('div');
  notice.className = 'notice-body';

  const headline = document.createElement('p');
  headline.className = 'headline';
  headline.textContent = moved.headline;
  notice.appendChild(headline);

  if (moved.grants_withdrawn > 0) {
    const caution = document.createElement('p');
    caution.className = 'caution';
    caution.textContent = `${moved.grants_withdrawn} permission(s) went with it.`;
    notice.appendChild(caution);
  }

  const note = document.createElement('p');
  note.className = 'note';
  note.textContent = moved.description;
  notice.appendChild(note);

  const dismiss = document.createElement('button');
  dismiss.className = 'dismiss';
  dismiss.textContent = 'Dismiss';
  notice.appendChild(dismiss);

  return notice;
}

/** Starts a generation run and watches it. */
async function beginGenerating(id) {
  try {
    const offered = await ask('providers');
    const provider = document.querySelector('select.provider')?.value ?? offered[0];

    await ask('generate', { id, provider });
    await open(id);
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

document.addEventListener('submit', (event) => {
  const form = event.target.closest('form.composer');
  if (form) {
    event.preventDefault();
    submitIntent(form);
  }
});

document.addEventListener('click', (event) => {
  const dismiss = event.target.closest('#notice button.dismiss');
  if (dismiss) {
    clearOutcome();
    return;
  }

  const back = event.target.closest('button.back');
  if (back) {
    refresh();
    return;
  }

  const button = event.target.closest('button[data-decision]');
  if (button) {
    const item = button.closest('li.permission');
    const typed = item.querySelector('input.confirm')?.value ?? 'allow';
    decide(item, button.dataset.decision === 'deny' ? 'deny' : typed);
    return;
  }

  // Rolling back is asked twice. It clears the build and can take permissions
  // away, and neither is undone by clicking again — so the first click says
  // what it costs and the second one does it, the way a critical permission
  // takes a typed word rather than a reflex.
  const offer = event.target.closest('button.rollback');
  if (offer) {
    const version = offer.closest('li.version');
    version
      .querySelector('.revert')
      .replaceWith(
        rollbackConfirm({
          digest: version.dataset.digest,
          sequence: Number(version.dataset.sequence),
        }),
      );
    return;
  }

  const cancel = event.target.closest('button.cancel-rollback');
  if (cancel) {
    // Straight back to the page as it was: re-reading it is what guarantees the
    // controls are the ones the current state calls for.
    open(cancel.closest('.detail').dataset.id);
    return;
  }

  const confirmed = event.target.closest('button.confirm-rollback');
  if (confirmed) {
    confirmed.disabled = true;
    rollback(confirmed.closest('.detail').dataset.id, confirmed.dataset.digest);
    return;
  }

  const authority = event.target.closest('button.grant-authority, button.revoke-authority');
  if (authority) {
    const entry = authority.closest('li.authority');
    const granting = authority.classList.contains('grant-authority');
    const typed = entry.querySelector('input.confirm')?.value ?? '';

    // The same rule the terminal holds, judged here rather than inferred from
    // which control was clicked: Ephemeral's own authority is never granted by
    // a stray click.
    if (granting && !isConsent({ needs_explicit_confirmation: true }, typed)) {
      reportProblem('Type `allow` to let Ephemeral do this. Nothing has been decided.');
      return;
    }

    decideAuthority(authority.dataset.capability, granting);
    return;
  }

  const act = event.target.closest('button.start, button.halt, button.generate, button.move, button.purge');
  if (act) {
    const page = act.closest('.detail');
    const id = page.dataset.id;

    if (act.classList.contains('start')) {
      start(id, page.querySelector('input.arguments')?.value ?? '');
    } else if (act.classList.contains('halt')) {
      simply('halt', { id }, id);
    } else if (act.classList.contains('generate')) {
      beginGenerating(id);
    } else if (act.classList.contains('purge')) {
      confirmDestruction(act, id);
    } else {
      moveApp(id, act.dataset.event);
    }
    return;
  }

  const confirmedPurge = event.target.closest('button.confirm-purge');
  if (confirmedPurge) {
    purge(confirmedPurge.dataset.id);
    return;
  }

  const acknowledged = event.target.closest('button.acknowledge');
  if (acknowledged) {
    const page = acknowledged.closest('.detail');
    ask('acknowledge', { id: page.dataset.id })
      .then(() => open(page.dataset.id))
      .catch((error) => reportProblem(String(error.message ?? error)));
    return;
  }

  const item = event.target.closest('li.application');
  if (item) open(item.dataset.id);
});

document.getElementById('compose').replaceChildren(composer());

refresh();

export { refresh, open, problem };
