// The desktop window's rendering, tested in a browser without a window.
//
// A desktop UI that could only be exercised by launching it is a UI nobody
// tests. These run against the same modules the real window loads, in headless
// Chromium, so the permission prompt somebody sees in a window is checked the
// same way the one in the terminal is.

import { chromium } from 'playwright';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join, extname } from 'node:path';
import assert from 'node:assert/strict';

const here = dirname(fileURLToPath(import.meta.url));
const ui = join(here, '..', 'ui');

const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' };

const server = createServer(async (request, response) => {
  const name = request.url === '/' ? '/index.html' : request.url;
  try {
    const body = await readFile(join(ui, name));
    response.writeHead(200, { 'content-type': TYPES[extname(name)] ?? 'text/plain' });
    response.end(body);
  } catch {
    response.writeHead(404).end('not found');
  }
});

await new Promise((resolve) => server.listen(0, resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
const page = await browser.newPage();
await page.goto(origin);

const failures = [];
async function check(name, body) {
  try {
    await body();
    console.log(`  ok  ${name}`);
  } catch (error) {
    failures.push(name);
    console.log(`FAIL  ${name}\n      ${error.message}`);
  }
}

const summary = (over = {}) => ({
  id: 'csv-comparator',
  name: 'CSV comparator',
  purpose: 'compare two CSV files',
  state: 'Ready',
  state_kind: 'idle',
  runnable: true,
  running: false,
  put_away: false,
  granted: 0,
  highest_granted_risk: null,
  awaiting_decision: 0,
  updated_at: '2026-08-15T12:00:00Z',
  ...over,
});

const permission = (over = {}) => ({
  capability: 'filesystem_read',
  wants: 'read the files in ~/Downloads',
  reason: 'to compare the files you selected',
  if_allowed: 'It can read what is at ~/Downloads.',
  risk: 'medium',
  needs_explicit_confirmation: false,
  revocable: true,
  ...over,
});

// An application waiting on a decision is what a person most needs to notice.
await check('a waiting decision is shown prominently', async () => {
  const text = await page.evaluate(async (data) => {
    const { applicationItem } = await import('./render.js');
    return applicationItem(data).textContent;
  }, summary({ awaiting_decision: 2 }));

  assert.match(text, /2 decisions waiting/);
});

// An application allowed to reach the whole internet was drawn on the list
// exactly like one that can see nothing of yours. Looking at a film of the
// window is what showed it; no test could, because both rendered correctly.
await check('the list says what an application already holds', async () => {
  const result = await page.evaluate(async (data) => {
    const { applicationItem } = await import('./render.js');
    const wide = applicationItem({ ...data, granted: 2, highest_granted_risk: 'critical' });
    const narrow = applicationItem({ ...data, granted: 1, highest_granted_risk: 'low' });
    const none = applicationItem(data);
    return {
      wide: wide.querySelector('.grants')?.textContent,
      wideRisk: wide.querySelector('.grants')?.className,
      narrow: narrow.querySelector('.grants')?.textContent,
      narrowRisk: narrow.querySelector('.grants')?.className,
      none: none.querySelector('.grants'),
      unknown: applicationItem({ ...data, granted: 1 }).querySelector('.grants')?.className,
    };
  }, summary());

  assert.match(result.wide, /Allowed 2 things/);
  assert.match(result.narrow, /Allowed 1 thing/);
  assert.notEqual(result.wideRisk, result.narrowRisk, 'and how much it holds');
  assert.match(result.wideRisk, /risk-critical/);
  assert.equal(result.none, null, 'an app holding nothing claims nothing');
  // Defaulting an unknown risk to `low` paints a reassuring green on an
  // application that might hold the widest permission there is.
  assert.doesNotMatch(result.unknown, /risk-/, 'an unknown risk is never drawn as a safe one');
});

await check('an application with nothing waiting says nothing about it', async () => {
  const text = await page.evaluate(async (data) => {
    const { applicationItem } = await import('./render.js');
    return applicationItem(data).textContent;
  }, summary());

  assert.doesNotMatch(text, /waiting/);
});

// "No applications yet" would be a lie when some are archived.
await check('archived applications are counted rather than vanishing', async () => {
  const text = await page.evaluate(async (data) => {
    const { applicationList } = await import('./render.js');
    return applicationList(data).textContent;
  }, [summary({ put_away: true })]);

  assert.match(text, /1 archived or deleted/);
  assert.doesNotMatch(text, /No applications yet/);
});

await check('an empty list says so plainly', async () => {
  const text = await page.evaluate(async () => {
    const { applicationList } = await import('./render.js');
    return applicationList([]).textContent;
  });

  assert.match(text, /No applications yet/);
});

// The reason is the only part of a request a person cannot check, so an absent
// one must look different from a good one.
await check('a request with no reason says so rather than staying silent', async () => {
  const text = await page.evaluate(async (data) => {
    const { permissionItem } = await import('./render.js');
    return permissionItem(data).textContent;
  }, permission({ reason: null }));

  assert.match(text, /gives no reason/);
});

await check('a stated reason is attributed rather than asserted', async () => {
  const text = await page.evaluate(async (data) => {
    const { permissionItem } = await import('./render.js');
    return permissionItem(data).textContent;
  }, permission());

  assert.match(text, /It says:/);
});

// A high-risk permission must not be accepted by the same reflex as a low one.
await check('a high-risk request is marked as needing confirmation', async () => {
  const marked = await page.evaluate(async (data) => {
    const { permissionItem } = await import('./render.js');
    const node = permissionItem(data);
    return { risk: node.dataset.risk, confirm: node.dataset.needsConfirmation };
  }, permission({ risk: 'critical', needs_explicit_confirmation: true }));

  assert.equal(marked.risk, 'critical');
  assert.equal(marked.confirm, 'true');
});

// The reassuring common case, said out loud.
await check('an isolated application says it can see nothing', async () => {
  const text = await page.evaluate(async () => {
    const { permissionsSection } = await import('./render.js');
    return permissionsSection({
      allowed: [],
      outstanding: [],
      highest_granted_risk: null,
      isolated: true,
    }).textContent;
  });

  assert.match(text, /can see nothing of yours/);
});

// Application names and model-written reasons are untrusted strings. There must
// be no route from either into markup.
await check('a hostile name cannot become markup', async () => {
  const result = await page.evaluate(async (data) => {
    const { applicationItem } = await import('./render.js');
    const node = applicationItem(data);
    return { html: node.innerHTML, text: node.textContent };
  }, summary({ name: '<img src=x onerror="window.__pwned=1">' }));

  assert.doesNotMatch(result.html, /<img/);
  assert.match(result.text, /onerror/);
  assert.equal(await page.evaluate(() => window.__pwned), undefined);
});

await check('a hostile reason cannot become markup either', async () => {
  const html = await page.evaluate(async (data) => {
    const { permissionItem } = await import('./render.js');
    return permissionItem(data).innerHTML;
  }, permission({ reason: '<script>window.__pwned=1</script>' }));

  assert.doesNotMatch(html, /<script>/);
  assert.equal(await page.evaluate(() => window.__pwned), undefined);
});

// The same rule the terminal holds: a critical permission takes the word, not a
// click. Two clients disagreeing about what counts as consent would be worse
// than either being wrong.
await check('a critical permission is not granted by a click', async () => {
  const result = await page.evaluate(async (data) => {
    const { isConsent, decisionControls } = await import('./render.js');
    return {
      click: isConsent(data, 'allow') && !data.needs_explicit_confirmation,
      typedWord: isConsent(data, 'allow'),
      typedWrong: isConsent(data, 'yes'),
      typedEmpty: isConsent(data, ''),
      hasButton: decisionControls(data).querySelector('button.allow') !== null,
      hasField: decisionControls(data).querySelector('input.confirm') !== null,
      hasSubmit: decisionControls(data).querySelector('button.confirm-allow') !== null,
    };
  }, permission({ risk: 'critical', needs_explicit_confirmation: true }));

  assert.equal(result.typedWord, true, 'the word allows it');
  assert.equal(result.typedWrong, false, '"yes" does not');
  assert.equal(result.typedEmpty, false, 'nor does nothing');
  assert.equal(result.hasButton, false, 'there is no one-click affirmative');
  assert.equal(result.hasField, true, 'there is a field to type into');
  // A field with nothing to submit it is a dead end, and the first recording of
  // this window had exactly that: somebody could type `allow` and nothing would
  // happen. Every test above passed while it was broken, because they all
  // asserted what was absent and none asserted what a person could do next.
  assert.equal(result.hasSubmit, true, 'and something to submit it with');
});

// Looking at that same recording showed a granted permission still offering
// "Allow", in the same colour as an unanswered one. Nothing on screen told the
// two apart.
await check('an allowed permission offers revocation, not approval', async () => {
  const result = await page.evaluate(async (data) => {
    const { permissionItem } = await import('./render.js');
    const held = permissionItem(data, { held: true });
    const asked = permissionItem(data);
    return {
      heldOffersRevoke: held.querySelector('button.revoke') !== null,
      heldOffersAllow: held.querySelector('button[data-decision="allow"]') !== null,
      heldIsMarked: held.classList.contains('held') && held.dataset.held === 'true',
      askedOffersAllow: asked.querySelector('button[data-decision="allow"]') !== null,
      askedIsMarked: asked.classList.contains('held'),
    };
  }, permission());

  assert.equal(result.heldOffersRevoke, true, 'what you allowed can be taken back');
  assert.equal(result.heldOffersAllow, false, 'and is not offered for approval again');
  assert.equal(result.heldIsMarked, true, 'and looks different from a request');
  assert.equal(result.askedOffersAllow, true, 'an open request is still answerable');
  assert.equal(result.askedIsMarked, false, 'and is not dressed as a grant');
});

// Granting something does not make it safer. An earlier version of the "held"
// styling recoloured a granted permission green and faded it, so an application
// allowed to reach the entire internet was drawn as the calmest thing on the
// page. Nothing caught it but looking at a film.
await check('allowing a permission does not make it look less dangerous', async () => {
  const shown = await page.evaluate(async (data) => {
    const { permissionItem } = await import('./render.js');
    const held = permissionItem(data, { held: true });
    const asked = permissionItem(data);
    document.body.append(held, asked);
    const colour = (node) => getComputedStyle(node.querySelector('.wants')).color;
    const result = { held: colour(held), asked: colour(asked), risk: held.className };
    held.remove();
    asked.remove();
    return result;
  }, permission({ risk: 'critical', needs_explicit_confirmation: true }));

  assert.match(shown.risk, /risk-critical/, 'a held permission keeps its risk class');
  assert.equal(shown.held, shown.asked, 'and is drawn in the same risk colour as when asked');
});

// Two unlabelled lists of capabilities next to each other tell a person nothing
// about which is which.
await check('what is held and what is asked for are labelled separately', async () => {
  const text = await page.evaluate(async (data) => {
    const { permissionsSection } = await import('./render.js');
    return permissionsSection({
      allowed: [data],
      outstanding: [{ ...data, capability: 'network_outbound' }],
      highest_granted_risk: 'medium',
      isolated: false,
    }).textContent;
  }, permission());

  assert.match(text, /1 thing you have allowed/);
  assert.match(text, /1 thing it is asking for/);
});

await check('an ordinary permission can be answered with a click', async () => {
  const result = await page.evaluate(async (data) => {
    const { isConsent, decisionControls } = await import('./render.js');
    return {
      allowed: isConsent(data, 'allow'),
      nothing: isConsent(data, ''),
      hasButton: decisionControls(data).querySelector('button.allow') !== null,
    };
  }, permission());

  assert.equal(result.allowed, true);
  assert.equal(result.nothing, false, 'silence is still not consent');
  assert.equal(result.hasButton, true);
});

// Making "no" harder than "yes" is how consent gets manufactured.
await check('refusing is always one click, whatever the risk', async () => {
  for (const risk of ['low', 'critical']) {
    const hasDeny = await page.evaluate(async (data) => {
      const { decisionControls } = await import('./render.js');
      return decisionControls(data).querySelector('button.deny') !== null;
    }, permission({ risk, needs_explicit_confirmation: risk === 'critical' }));

    assert.ok(hasDeny, `${risk} must be refusable in one click`);
  }
});

// Whether data leaves the device is its own line, not buried in prose.
await check('a remote runtime is marked differently from a local one', async () => {
  const classes = await page.evaluate(async () => {
    const { applicationDetail } = await import('./render.js');
    const base = {
      summary: {
        id: 'x', name: 'X', purpose: '', state: 'Ready', state_kind: 'idle',
        runnable: true, running: false, put_away: false, granted: 0,
        awaiting_decision: 0, updated_at: '2026-08-15T12:00:00Z',
      },
      explanation: 'ready', description: 'ready',
      limits: { description: 'small', cpu_millis: 500, memory_mib: 512, storage_mib: 1024 },
      permissions: { allowed: [], outstanding: [], highest_granted_risk: null, isolated: true },
      versions: [], retention: 'a week',
    };
    const runtime = (runs_locally) => ({
      kind: 'docker', isolation: 'text', runs_locally,
      image: null, interface: 'job', primary_action: 'Run once',
    });

    const local = applicationDetail({ ...base, runtime: runtime(true) });
    const remote = applicationDetail({ ...base, runtime: runtime(false) });
    return {
      local: local.querySelector('.local') !== null,
      remote: remote.querySelector('.remote') !== null,
    };
  });

  assert.ok(classes.local, 'a local runtime is marked local');
  assert.ok(classes.remote, 'a remote one is marked remote');
});

// The frontend reaches Rust through `window.__TAURI__`, which Tauri v2 injects
// only when `withGlobalTauri` is set. Without it the window opens, renders its
// header, and says "This window is not running inside Ephemeral" — while
// running inside Ephemeral. It shipped that way. Nothing here could see it:
// the rendering was correct, the commands were correct, and the two were never
// connected. Filming the real window under a virtual display found it on the
// first frame.
//
// Asserted here rather than only in `src-tauri`, because that is its own
// workspace and CI never builds it — a guard in a crate nobody compiles guards
// nothing.
await check('the frontend can reach Rust at all', async () => {
  const configuration = JSON.parse(
    await readFile(join(here, '..', 'src-tauri', 'tauri.conf.json'), 'utf8'),
  );

  assert.equal(
    configuration.app?.withGlobalTauri,
    true,
    'app.js calls window.__TAURI__ and there is no bundler to import from',
  );
});

// Everything above tests a pure function, and everything above passed while the
// window was refusing a critical permission with a message rendered hundreds of
// pixels below where the person was looking. A refusal nobody can see is the
// same as no refusal, and no assertion about `textContent` can tell the
// difference. This one drives the real page and asks where things ended up.
await check('a refusal is on screen, not below the fold', async () => {
  const stubbed = await browser.newPage();
  await stubbed.setViewportSize({ width: 900, height: 700 });

  const critical = permission({
    capability: 'network_outbound',
    wants: 'connect to anywhere on the internet',
    if_allowed: 'It can send anything it can read to anywhere.',
    risk: 'critical',
    needs_explicit_confirmation: true,
  });

  // Enough allowed permissions above it to push the request well past the fold,
  // which is the ordinary case for an application that has been used at all.
  await stubbed.addInitScript(
    ({ asked, held, app }) => {
      window.__TAURI__ = {
        core: {
          invoke: async (command) => {
            if (command === 'applications') return [app];
            if (command === 'application') {
              return {
                summary: app,
                explanation: 'ready',
                description: 'ready',
                runtime: null,
                limits: { description: 'small', cpu_millis: 500, memory_mib: 512, storage_mib: 1024 },
                permissions: {
                  allowed: held,
                  outstanding: [asked],
                  highest_granted_risk: 'medium',
                  isolated: false,
                },
                versions: [],
                retention: 'a week',
              };
            }
            throw new Error(`no such command: ${command}`);
          },
        },
      };
    },
    {
      asked: critical,
      held: [0, 1, 2, 3].map((n) => ({ ...permission(), capability: `filesystem_read_${n}` })),
      app: summary({ granted: 4, highest_granted_risk: 'medium', awaiting_decision: 1 }),
    },
  );

  await stubbed.goto(origin);
  await stubbed.click('li.application');

  const field = 'li.permission[data-capability="network_outbound"] input.confirm';
  await stubbed.fill(field, 'yes');
  await stubbed.click('li.permission[data-capability="network_outbound"] button.confirm-allow');
  await stubbed.waitForSelector('#problem:not([hidden])');

  const seen = await stubbed.evaluate(() => {
    const box = document.getElementById('problem').getBoundingClientRect();
    return {
      text: document.getElementById('problem').textContent,
      onScreen:
        box.top >= 0 &&
        box.bottom <= window.innerHeight &&
        box.left >= 0 &&
        box.right <= window.innerWidth,
    };
  });

  assert.match(seen.text, /Nothing has been decided/);
  assert.ok(seen.onScreen, 'the refusal must be inside the viewport, wherever the page is');

  await stubbed.close();
});

await browser.close();
server.close();

console.log(`\n${failures.length === 0 ? 'All rendering checks passed.' : `${failures.length} failed.`}`);
process.exit(failures.length === 0 ? 0 : 1);
