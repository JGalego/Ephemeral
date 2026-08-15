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

async function refresh() {
  try {
    const summaries = await ask('applications');
    show(applicationList(summaries));
    document.getElementById('problem').hidden = true;
  } catch (error) {
    // The message from the core is already written for a person; adding to it
    // would be inventing detail this layer does not have.
    reportProblem(String(error.message ?? error));
  }
}

async function open(id) {
  try {
    const detail = await ask('application', { id });
    const panel = document.getElementById('detail');
    panel.replaceChildren(applicationDetail(detail));
    panel.hidden = false;
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
    await open(page.dataset.id);
    await refresh();
  } catch (error) {
    reportProblem(String(error.message ?? error));
  }
}

document.addEventListener('click', (event) => {
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
