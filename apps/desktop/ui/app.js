// The desktop window.
//
// Everything that decides what to show lives in `render.js`, which is pure and
// tested. This file does one thing: ask Ephemeral for a view and hand it over.
// It decides nothing — a client that evaluated a permission or computed a
// transition would be a second, subtly different Ephemeral.

import {
  applicationList,
  applicationDetail,
  composer,
  isConsent,
  problem,
  rollbackConfirm,
  rollbackNotice,
} from './render.js';

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
  const summaries = await ask('applications');
  show(applicationList(summaries));
  document.getElementById('problem').hidden = true;
}

/** Goes to the list. */
async function refresh() {
  try {
    clearOutcome();
    await reload();
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
    const detail = await ask('application', { id });
    const panel = document.getElementById('detail');
    panel.replaceChildren(applicationDetail(detail));
    panel.hidden = false;
    // Replace the list rather than stacking beneath it. The first recording of
    // this window showed both at once, which reads as two pages at the same
    // time.
    document.getElementById('applications').hidden = true;
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

  const item = event.target.closest('li.application');
  if (item) open(item.dataset.id);
});

document.getElementById('compose').replaceChildren(composer());

refresh();

export { refresh, open, problem };
