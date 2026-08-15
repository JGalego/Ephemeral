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
    };
  }, permission({ risk: 'critical', needs_explicit_confirmation: true }));

  assert.equal(result.typedWord, true, 'the word allows it');
  assert.equal(result.typedWrong, false, '"yes" does not');
  assert.equal(result.typedEmpty, false, 'nor does nothing');
  assert.equal(result.hasButton, false, 'there is no one-click affirmative');
  assert.equal(result.hasField, true, 'there is a field to type into');
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

await browser.close();
server.close();

console.log(`\n${failures.length === 0 ? 'All rendering checks passed.' : `${failures.length} failed.`}`);
process.exit(failures.length === 0 ? 0 : 1);
