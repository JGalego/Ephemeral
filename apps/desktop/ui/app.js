// The desktop window.
//
// Everything that decides what to show lives in `render.js`, which is pure and
// tested. This file does one thing: ask Ephemeral for a view and hand it over.
// It decides nothing — a client that evaluated a permission or computed a
// transition would be a second, subtly different Ephemeral.

import { applicationList, applicationDetail, isConsent, problem } from './render.js';

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
    await reload();
    document.getElementById('applications').hidden = false;
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

document.addEventListener('click', (event) => {
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

  const item = event.target.closest('li.application');
  if (item) open(item.dataset.id);
});

refresh();

export { refresh, open, problem };
