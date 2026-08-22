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

const version = (over = {}) => ({
  digest: 'a1b2c3d4e5f6',
  sequence: 2,
  reason: 'generated',
  created_at: '2026-08-15T12:00:00Z',
  current: false,
  source_kept: true,
  ...over,
});

const permission = (over = {}) => ({
  capability: 'filesystem_read',
  effective: true,
  blocked_by: null,
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

// The window could show applications and answer their questions but not start
// one, so the first thing anybody had to do with a graphical application was
// open a terminal. These drive the real page: the composer only matters as
// something a person can actually type into and submit.
await check('somebody can ask for an application without a terminal', async () => {
  const stubbed = await browser.newPage();
  const asked = [];

  await stubbed.addInitScript((app) => {
    window.__asked = [];
    let created = false;
    window.__TAURI__ = {
      core: {
        invoke: async (command, args) => {
          window.__asked.push({ command, args });
          if (command === 'applications') return created ? [app] : [];
          if (command === 'create') {
            created = true;
            return app;
          }
          if (command === 'application') {
            return {
              summary: app,
              explanation: 'Requested. Nothing has been written yet.',
              description: app.purpose,
              runtime: null,
              limits: { description: 'small', cpu_millis: 500, memory_mib: 512, storage_mib: 1024 },
              permissions: { allowed: [], outstanding: [], highest_granted_risk: null, isolated: true },
              versions: [],
              retention: 'a week',
            };
          }
          throw new Error(`no such command: ${command}`);
        },
      },
    };
  }, summary({ state: 'Requested', state_kind: 'working' }));

  await stubbed.goto(origin);
  await stubbed.fill('textarea.intent', 'compare these two CSV files');
  await stubbed.click('form.composer button.create');
  await stubbed.waitForSelector('#detail:not([hidden])');

  const seen = await stubbed.evaluate(() => ({
    sent: window.__asked.filter((call) => call.command === 'create'),
    // The field is cleared, so the next thing typed is not appended to the
    // last thing asked for.
    field: document.querySelector('textarea.intent').value,
    // The composer is not left hovering over the page it just opened, offering
    // a new application instead of the one in front of the person.
    composerVisible: !document.getElementById('compose').hidden,
    detail: document.getElementById('detail').textContent,
  }));

  assert.equal(seen.sent.length, 1, 'exactly one application should have been created');
  assert.equal(seen.sent[0].args.intent, 'compare these two CSV files');
  assert.equal(seen.field, '', 'the field should be cleared after creating');
  assert.equal(seen.composerVisible, false, 'the composer should not sit over the new page');
  assert.match(seen.detail, /Nothing has been written yet/);

  asked.push(...seen.sent);
  await stubbed.close();
});

// An application recorded is not an application built, and somebody who is not
// told that will wait for a build nobody started. The same honesty the mobile
// library owes its host, a window owes the person in front of it.
await check('the composer says that asking is not building', async () => {
  const text = await page.evaluate(async () => {
    const { composer } = await import('./render.js');
    return composer().textContent;
  });

  assert.match(text, /Nothing is written or run until you generate it/);
});

// A second click before the first returns would create a second application
// nobody asked for — and creating touches the disk, so it would really be
// there.
await check('creating cannot be double-submitted', async () => {
  const stubbed = await browser.newPage();

  await stubbed.addInitScript((app) => {
    window.__creates = 0;
    window.__TAURI__ = {
      core: {
        invoke: async (command) => {
          if (command === 'applications') return [];
          if (command === 'create') {
            window.__creates += 1;
            // Slow, the way a disk write under load is slow.
            await new Promise((resolve) => setTimeout(resolve, 300));
            return app;
          }
          if (command === 'application') throw new Error('not reached');
          throw new Error(`no such command: ${command}`);
        },
      },
    };
  }, summary());

  await stubbed.goto(origin);
  await stubbed.fill('textarea.intent', 'count the words in a file');

  const button = 'form.composer button.create';
  await stubbed.click(button);
  const disabled = await stubbed.getAttribute(button, 'disabled');
  await stubbed.click(button, { force: true }).catch(() => {});
  await stubbed.waitForTimeout(500);

  const creates = await stubbed.evaluate(() => window.__creates);
  assert.notEqual(disabled, null, 'the button must be disabled while a creation is in flight');
  assert.equal(creates, 1, 'a second click created a second application');

  await stubbed.close();
});

// An intent the terminal would refuse must not be accepted by the window, and
// the reason shown has to be the core's own words rather than the window's.
await check('an intent Ephemeral refuses is refused here too', async () => {
  const stubbed = await browser.newPage();

  await stubbed.addInitScript(() => {
    window.__TAURI__ = {
      core: {
        invoke: async (command) => {
          if (command === 'applications') return [];
          if (command === 'create') {
            throw new Error('tell me what you want the application to do');
          }
          throw new Error(`no such command: ${command}`);
        },
      },
    };
  });

  await stubbed.goto(origin);
  await stubbed.click('form.composer button.create');
  await stubbed.waitForSelector('#problem:not([hidden])');

  const seen = await stubbed.evaluate(() => ({
    text: document.getElementById('problem').textContent,
    // Usable again: a window that stays disabled after a refusal has stopped
    // being a window.
    disabled: document.querySelector('form.composer button.create').disabled,
  }));

  assert.match(seen.text, /tell me what you want the application to do/);
  assert.equal(seen.disabled, false, 'the button must come back after a refusal');

  await stubbed.close();
});

// A capability the person allowed that Ephemeral itself may not carry out is
// held and does nothing. Drawing it as active would put authority on the screen
// that the sandbox does not give; hiding it would erase a decision they made.
await check('a permission Ephemeral cannot carry out is shown as doing nothing', async () => {
  const seen = await page.evaluate(async (data) => {
    const { permissionsSection } = await import('./render.js');
    const section = permissionsSection({
      isolated: true,
      allowed: [
        {
          ...data,
          effective: false,
          blocked_by: 'read the files in ~/Downloads',
        },
      ],
      outstanding: [],
      highest_granted_risk: null,
    });

    const item = section.querySelector('li.permission');
    return {
      heading: section.querySelector('h3.holds')?.textContent,
      inert: item.classList.contains('inert'),
      effective: item.dataset.effective,
      blocked: item.querySelector('.blocked')?.textContent,
      offersRevoke: item.querySelector('button.revoke') !== null,
      isolated: section.querySelector('.isolated')?.textContent,
    };
  }, permission());

  assert.match(seen.heading, /does nothing yet|do nothing yet/);
  assert.equal(seen.inert, true);
  assert.equal(seen.effective, 'false');
  assert.match(seen.blocked, /Ephemeral itself has not been allowed/);
  assert.match(seen.blocked, /read the files/);
  assert.equal(seen.offersRevoke, true, 'the decision is still theirs to take back');
  assert.match(seen.isolated, /can see nothing of yours/);
});

// And one that works must not be drawn as dormant, or the marking means
// nothing.
await check('a permission that works is not marked as inert', async () => {
  const seen = await page.evaluate(async (data) => {
    const { permissionItem } = await import('./render.js');
    const item = permissionItem({ ...data, effective: true, blocked_by: null }, { held: true });
    return {
      inert: item.classList.contains('inert'),
      effective: item.dataset.effective,
      blocked: item.querySelector('.blocked'),
    };
  }, permission());

  assert.equal(seen.inert, false);
  assert.equal(seen.effective, 'true');
  assert.equal(seen.blocked, null);
});

// Rolling back needs a version to roll back to, and the page said nothing about
// versions at all: everything an application had been was visible in the
// terminal and nowhere in the window.
await check('the page lists what an application has been', async () => {
  const seen = await page.evaluate(
    async ([one, two]) => {
      const { versionsSection } = await import('./render.js');
      const section = versionsSection([two, one]);
      return {
        heading: section.querySelector('h3.history')?.textContent,
        entries: [...section.querySelectorAll('li.version')].map((item) => item.textContent),
        first: section.querySelector('li.version')?.dataset.digest,
      };
    },
    [version(), version({ digest: 'ffff1111', sequence: 3, current: true })],
  );

  assert.match(seen.heading, /2 versions/);
  assert.equal(seen.entries.length, 2);
  assert.match(seen.entries[0], /Version 3/);
  assert.equal(seen.first, 'ffff1111', 'newest first, as the service layer ordered them');
});

// Three states, three different offers. The one that matters is the middle one:
// a version can be in the history with its source gone, and a button offering
// to return to it is a button that cannot work.
await check('only a version that can be returned to offers to be', async () => {
  const seen = await page.evaluate(
    async ([kept, gone, unknown, now]) => {
      const { versionItem } = await import('./render.js');
      const rendered = (data) => {
        const item = versionItem(data);
        return {
          offers: item.querySelector('button.rollback') !== null,
          text: item.textContent,
        };
      };
      return {
        kept: rendered(kept),
        gone: rendered(gone),
        unknown: rendered(unknown),
        now: rendered(now),
      };
    },
    [
      version(),
      version({ source_kept: false }),
      version({ source_kept: null }),
      version({ current: true }),
    ],
  );

  assert.equal(seen.kept.offers, true);
  assert.equal(seen.gone.offers, false, 'a version whose source is gone cannot be returned to');
  assert.match(seen.gone.text, /nothing to go back to/);
  assert.equal(seen.unknown.offers, false, 'an unchecked answer is not a yes');
  assert.equal(seen.now.offers, false, 'the version it is on is not somewhere to go');
  assert.match(seen.now.text, /version it is on now/);
});

// Rolling back clears the build and can take permissions away, and clicking
// again does not undo either. The cost is said before the second click, the way
// a critical permission takes a typed word rather than a reflex.
await check('rolling back is asked twice, and says what it costs first', async () => {
  const seen = await page.evaluate(async (data) => {
    const { rollbackConfirm } = await import('./render.js');
    const controls = rollbackConfirm(data);
    return {
      text: controls.querySelector('.consequence')?.textContent,
      confirms: controls.querySelector('button.confirm-rollback')?.dataset.digest,
      cancels: controls.querySelector('button.cancel-rollback') !== null,
    };
  }, version());

  assert.match(seen.text, /Version 2|version 2/);
  assert.match(seen.text, /image is cleared/);
  assert.match(seen.text, /taken back/, 'the permissions it costs are said before the click');
  assert.equal(seen.confirms, 'a1b2c3d4e5f6');
  assert.equal(seen.cancels, true, 'saying no must be one click');
});

// What a rollback took away is the sentence somebody has to act on, and it
// arrives in the service layer's words so the window cannot report it
// differently from the terminal.
await check('what a rollback did is shown in the words the service layer used', async () => {
  const seen = await page.evaluate(async (done) => {
    const { rollbackNotice } = await import('./render.js');
    const notice = rollbackNotice(done);
    return {
      headline: notice.querySelector('.headline')?.textContent,
      caution: notice.querySelector('.caution')?.textContent,
      note: notice.querySelector('.note')?.textContent,
      dismissable: notice.querySelector('button.dismiss') !== null,
      quiet: rollbackNotice({ ...done, caution: null }).querySelector('.caution'),
    };
  }, {
    app: 'csv-comparator',
    sequence: 1,
    digest: 'a1b2c3d4e5f6',
    grants_withdrawn: 1,
    newly_requested: 1,
    headline: 'Rolled csv-comparator back to version 1 (a1b2c3d4e5f6).',
    caution: 'This version asks for 1 thing(s) the one it replaced had stopped needing.',
    note: 'The built image was cleared.',
  });

  assert.match(seen.headline, /Rolled csv-comparator back to version 1/);
  assert.match(seen.caution, /stopped needing/);
  assert.match(seen.note, /built image was cleared/);
  assert.equal(seen.quiet, null, 'a caution nobody needs is a caution nobody reads');
  assert.equal(seen.dismissable, true, 'a banner pinned over the page has to be dismissable');
});

// The window is the asking half. What it must not do is compose a rollback of
// its own: the digest it sends is one the service layer gave it.
await check('the window rolls back through the service layer and shows what it said', async () => {
  const stubbed = await browser.newPage();

  await stubbed.addInitScript(
    ([app, restorable]) => {
      window.__asked = [];
      const detail = {
        summary: app,
        explanation: 'It is ready.',
        description: 'Built, validated and available to run.',
        runtime: null,
        limits: { description: 'half a core', cpu_millis: 500, memory_mib: 512, storage_mib: 512 },
        permissions: { isolated: true, allowed: [], outstanding: [] },
        versions: [
          { ...restorable, sequence: 2, digest: 'ffff1111', current: true },
          restorable,
        ],
        retention: 'a week',
      };

      window.__TAURI__ = {
        core: {
          invoke: async (command, args) => {
            window.__asked.push([command, args]);
            if (command === 'applications') return [app];
            if (command === 'application') return detail;
            if (command === 'rollback') {
              return {
                app: app.id,
                sequence: 1,
                digest: args.version,
                grants_withdrawn: 2,
                newly_requested: 1,
                headline: `Rolled ${app.id} back to version 1 (${args.version}).`,
                caution: 'Two grants were withdrawn.',
                note: 'The built image was cleared.',
              };
            }
            throw new Error(`no such command: ${command}`);
          },
        },
      };
    },
    [summary(), version()],
  );

  await stubbed.goto(origin);
  await stubbed.click('li.application');
  await stubbed.waitForSelector('li.version button.rollback');

  // One click offers; it must not have rolled anything back yet.
  await stubbed.click('li.version button.rollback');
  const askedAfterFirstClick = await stubbed.evaluate(() =>
    window.__asked.filter(([command]) => command === 'rollback').length,
  );

  await stubbed.click('button.confirm-rollback');
  await stubbed.waitForSelector('#notice:not([hidden])');

  const seen = await stubbed.evaluate(() => ({
    rollbacks: window.__asked.filter(([command]) => command === 'rollback'),
    notice: document.getElementById('notice').textContent,
    problem: document.getElementById('problem').hidden,
  }));

  assert.equal(askedAfterFirstClick, 0, 'the first click asks, it does not act');
  assert.equal(seen.rollbacks.length, 1);
  assert.equal(seen.rollbacks[0][1].version, 'a1b2c3d4e5f6', 'the digest is the one it was given');
  assert.match(seen.notice, /Rolled csv-comparator back to version 1/);
  assert.match(seen.notice, /Two grants were withdrawn/, 'the caution has to survive the redraw');
  assert.equal(seen.problem, true, 'a rollback that worked is not a problem');

  // Pinned over the page, so it has to be possible to put down.
  await stubbed.click('#notice button.dismiss');
  assert.equal(
    await stubbed.evaluate(() => document.getElementById('notice').hidden),
    true,
    'dismissing puts the notice away',
  );

  await stubbed.close();
});

// A refusal from the service layer is the person's answer, in its words. The
// window must not turn "there is nothing to go back to" into a stack trace, and
// must not leave a notice claiming something happened.
await check('a refused rollback says why, in the core\'s own words', async () => {
  const stubbed = await browser.newPage();

  await stubbed.addInitScript(
    ([app, restorable]) => {
      const detail = {
        summary: app,
        explanation: 'It is ready.',
        description: 'Built, validated and available to run.',
        runtime: null,
        limits: { description: 'half a core', cpu_millis: 500, memory_mib: 512, storage_mib: 512 },
        permissions: { isolated: true, allowed: [], outstanding: [] },
        versions: [{ ...restorable, sequence: 2, digest: 'ffff1111', current: true }, restorable],
        retention: 'a week',
      };

      window.__TAURI__ = {
        core: {
          invoke: async (command) => {
            if (command === 'applications') return [app];
            if (command === 'application') return detail;
            if (command === 'rollback') {
              throw new Error(
                'version a1b2c3d4e5f6 of csv-comparator is recorded but its source is not on ' +
                  'this machine, so there is nothing to go back to.',
              );
            }
            throw new Error(`no such command: ${command}`);
          },
        },
      };
    },
    [summary(), version()],
  );

  await stubbed.goto(origin);
  await stubbed.click('li.application');
  await stubbed.click('li.version button.rollback');
  await stubbed.click('button.confirm-rollback');
  await stubbed.waitForSelector('#problem:not([hidden])');

  const seen = await stubbed.evaluate(() => ({
    problem: document.getElementById('problem').textContent,
    notice: document.getElementById('notice').hidden,
  }));

  assert.match(seen.problem, /nothing to go back to/);
  assert.equal(seen.notice, true, 'nothing happened, so nothing is announced');

  await stubbed.close();
});

// Everything the terminal can do, the window has to offer — and only what the
// application can actually do right now. Anything else is absent rather than
// disabled: a greyed-out row is a puzzle, and the state is already in words.
await check('the page offers what this application can actually do', async () => {
  const seen = await page.evaluate(async ([base, detail]) => {
    const { actions } = await import('./render.js');
    const labels = (over, runtime = detail.runtime) =>
      [
        ...actions({ ...detail, runtime, summary: { ...base, ...over } }).querySelectorAll(
          'button',
        ),
      ].map((button) => button.textContent);

    return {
      ready: labels({}),
      running: labels({ running: true, runnable: true, state_kind: 'running' }),
      // Never generated: it has no runtime at all, which is how the page
      // knows there is nothing built to run.
      requested: labels({ runnable: false, state_kind: 'working' }, null),
      archived: labels({ runnable: false, put_away: true, state_kind: 'archived' }),
      deleted: labels({ runnable: false, put_away: true, state_kind: 'deleted' }),
    };
  }, [summary(), { runtime: { kind: 'docker' }, providers: ['mock'] }]);

  assert.deepEqual(seen.ready, ['Run', 'Archive', 'Delete']);
  assert.deepEqual(seen.running, ['Stop', 'Delete'], 'a running application is not archived');
  assert.ok(seen.requested.includes('Generate'), 'nothing built yet, so it offers to build');
  assert.ok(seen.archived.includes('Restore'));
  assert.ok(seen.deleted.includes('Purge for good'), 'and the last step is its own button');
});

// Generation takes minutes. The window says so, says where it has got to in the
// lifecycle's own words, and does not invent a progress bar for something
// nothing measures.
await check('a generation run says where it has got to, in words that are true', async () => {
  const seen = await page.evaluate(async () => {
    const { generationPanel } = await import('./render.js');

    const running = generationPanel({ running: true }, 'Ephemeral is writing the app.');
    const built = generationPanel({
      running: false,
      built: {
        headline: 'Built. csv-comparator',
        how_it_went: 'Built first time, 2400 tokens in, 800 out',
        version: '8a5426d900da',
        requests: [{ wants: 'read the files in ~/Downloads', risk: 'medium' }],
        widened: null,
        grants_withdrawn: 0,
        unchanged: null,
        warnings: [],
      },
    });
    const failed = generationPanel({ running: false, failed: 'anthropic failed: no API key.' });

    return {
      running: running.textContent,
      runningClass: running.className,
      built: built.textContent,
      failed: failed.textContent,
      dismissable: built.querySelector('button.acknowledge') !== null,
      noBar: running.querySelector('progress') === null,
    };
  });

  assert.match(seen.running, /writing the app/);
  assert.match(seen.running, /takes minutes/);
  assert.match(seen.runningClass, /running/);
  assert.equal(seen.noBar, true, 'nothing measures this, so nothing pretends to');
  assert.match(seen.built, /Built\. csv-comparator/);
  assert.match(seen.built, /will ask for 1 thing\. It holds none of it yet/);
  assert.match(seen.failed, /no API key/);
  assert.equal(seen.dismissable, true);
});

// Output is written by generated code. It is placed as text, never as markup —
// the same rule the names and reasons follow, for the same reason.
await check('what an application printed cannot become markup', async () => {
  const seen = await page.evaluate(async () => {
    const { logsSection } = await import('./render.js');
    const section = logsSection({
      history: [{ at: '2026-08-22T10:00:00Z', state: 'Ready', from: 'Validating', what: 'it built' }],
      output: '<img src=x onerror="window.__owned = true">',
    });

    return {
      html: section.querySelector('pre.output').innerHTML,
      text: section.querySelector('pre.output').textContent,
      images: section.querySelectorAll('img').length,
      history: section.querySelector('ul.history').textContent,
    };
  });

  assert.equal(seen.images, 0, 'output is text, whatever it contains');
  assert.doesNotMatch(seen.html, /<img/);
  assert.match(seen.text, /onerror/);
  assert.match(seen.history, /it built/);
});

// The absence of output and the emptiness of it are different facts, and a
// person debugging a crash needs to know which one they are looking at.
await check('no container to ask reads differently from nothing printed', async () => {
  const seen = await page.evaluate(async () => {
    const { logsSection } = await import('./render.js');
    return {
      gone: logsSection({ history: [], output: null }).textContent,
      quiet: logsSection({ history: [], output: '   ' }).textContent,
    };
  });

  assert.match(seen.gone, /no container to ask/);
  assert.match(seen.quiet, /has not printed anything/);
});

// Ephemeral's own authority is the most powerful consent in the product: it
// outlives every application and covers all of them at once. It is never one
// click, and a window may not compose one out of a text field.
await check("Ephemeral's own authority is asked for separately, and typed", async () => {
  const seen = await page.evaluate(async () => {
    const { authoritySection } = await import('./render.js');
    const section = authoritySection([
      {
        capability: 'docker',
        wants: 'use Docker to run your apps in containers',
        if_allowed: 'It can build and run applications.',
        granted: false,
        risk: 'high',
        needs_explicit_confirmation: true,
        grantable: true,
      },
      {
        capability: 'read:~/Downloads/**',
        wants: 'read the files in ~/Downloads',
        if_allowed: 'It can read what is there.',
        granted: true,
        risk: 'medium',
        needs_explicit_confirmation: true,
        grantable: false,
      },
    ]);

    const items = [...section.querySelectorAll('li.authority')];
    return {
      heading: section.querySelector('h2.name').textContent,
      note: section.querySelector('p.note').textContent,
      offered: {
        typed: items[0].querySelector('input.confirm') !== null,
        button: items[0].querySelector('button.grant-authority')?.textContent,
        plainAllow: items[0].querySelector('button.allow') === null,
      },
      scoped: {
        revocable: items[1].querySelector('button.revoke-authority') !== null,
        grantable: items[1].querySelector('button.grant-authority') === null,
        advice: items[1].querySelector('.note')?.textContent,
      },
    };
  });

  assert.match(seen.heading, /What Ephemeral itself may do/);
  assert.match(seen.note, /never inherited in either direction/);
  assert.equal(seen.offered.typed, true, 'this authority is never one click');
  assert.equal(seen.offered.button, 'Confirm');
  assert.equal(seen.offered.plainAllow, true);
  assert.equal(seen.scoped.revocable, true, 'what is held can always be taken back');
  assert.equal(seen.scoped.grantable, true, 'a window may not compose a path, so it is not offered');
  assert.match(
    seen.scoped.advice,
    /Granting this again means naming the path/,
    'and somebody about to take it back is told that before they do',
  );
});

// The whole point of Phase 4: a person can generate and run without a terminal.
// Driven through the real frontend, against stubbed commands shaped exactly as
// the Rust side returns them.
await check('somebody can generate and run an application without a terminal', async () => {
  const stubbed = await browser.newPage();

  await stubbed.addInitScript((app) => {
    window.__asked = [];
    let generated = false;

    const detail = (over = {}) => ({
      summary: { ...app, ...over },
      explanation: over.explanation ?? 'Ephemeral is writing the app.',
      description: 'Working',
      runtime: over.built ? { kind: 'docker', isolation: 'in a container', runs_locally: true } : null,
      limits: { description: 'half a core', cpu_millis: 500, memory_mib: 512, storage_mib: 512 },
      permissions: { isolated: true, allowed: [], outstanding: [] },
      versions: [],
      retention: 'a week',
    });

    window.__TAURI__ = {
      core: {
        invoke: async (command, args) => {
          window.__asked.push([command, args]);
          if (command === 'applications') return [app];
          if (command === 'providers') return ['mock', 'local'];
          if (command === 'activity' || command === 'diagnostics' || command === 'authority') return [];
          if (command === 'logs') return { history: [], output: null };
          if (command === 'application') {
            return generated
              ? detail({ built: true, runnable: true, state: 'Ready', state_kind: 'idle' })
              : detail({ runnable: false, state: 'Writing the app', state_kind: 'working' });
          }
          if (command === 'generate') {
            generated = true;
            return null;
          }
          if (command === 'generation') {
            return generated
              ? {
                  running: false,
                  built: {
                    headline: 'Built. csv-comparator',
                    how_it_went: 'Built first time',
                    version: 'abc123',
                    requests: [],
                    widened: null,
                    grants_withdrawn: 0,
                    unchanged: null,
                    warnings: [],
                  },
                  failed: null,
                }
              : null;
          }
          if (command === 'start') {
            return {
              state: 'Running',
              container: 'abc123456789',
              confinement: ['Can read /srv/listings, which it sees as /mnt/srv-listings'],
              refused: [],
              inert: null,
            };
          }
          throw new Error(`no such command: ${command}`);
        },
      },
    };
  }, summary({ runnable: false, state: 'Requested', state_kind: 'working' }));

  await stubbed.goto(origin);
  await stubbed.click('li.application');
  await stubbed.waitForSelector('button.generate');

  await stubbed.selectOption('select.provider', 'mock');
  await stubbed.click('button.generate');
  await stubbed.waitForSelector('.generation.built');

  const generation = await stubbed.evaluate(() =>
    window.__asked.find(([command]) => command === 'generate'),
  );

  // Built, so the page now offers to run it — with somewhere to put arguments.
  await stubbed.waitForSelector('form.run input.arguments');
  await stubbed.fill('form.run input.arguments', '/mnt/srv-listings/left.csv /mnt/srv-listings/right.csv');
  await stubbed.click('button.start');
  await stubbed.waitForSelector('#notice:not([hidden])');

  const seen = await stubbed.evaluate(() => ({
    start: window.__asked.find(([command]) => command === 'start'),
    notice: document.getElementById('notice').textContent,
    problem: document.getElementById('problem').hidden,
  }));

  assert.equal(generation[1].provider, 'mock', 'the provider it was told to use');
  assert.deepEqual(
    seen.start[1].arguments,
    ['/mnt/srv-listings/left.csv', '/mnt/srv-listings/right.csv'],
    'the arguments are the application\'s own, passed as typed',
  );
  assert.match(seen.notice, /Started/);
  assert.match(seen.notice, /sees as \/mnt\/srv-listings/, 'and what it can reach is said');
  assert.equal(seen.problem, true);

  await stubbed.close();
});

// Granting Ephemeral its own authority is the most consequential click in the
// window, so a stray one must not do it.
await check("Ephemeral's authority is not granted by a stray click", async () => {
  const stubbed = await browser.newPage();

  await stubbed.addInitScript(() => {
    window.__decisions = [];
    window.__TAURI__ = {
      core: {
        invoke: async (command, args) => {
          if (command === 'applications') return [];
          if (command === 'diagnostics' || command === 'activity') return [];
          if (command === 'authority') {
            return [
              {
                capability: 'docker',
                wants: 'use Docker to run your apps in containers',
                if_allowed: 'It can build and run applications.',
                granted: false,
                risk: 'high',
                needs_explicit_confirmation: true,
                grantable: true,
              },
            ];
          }
          if (command === 'decide_authority') {
            window.__decisions.push(args);
            return null;
          }
          throw new Error(`no such command: ${command}`);
        },
      },
    };
  });

  await stubbed.goto(origin);
  await stubbed.waitForSelector('li.authority button.grant-authority');

  // The wrong word grants nothing.
  await stubbed.fill('li.authority input.confirm', 'yes');
  await stubbed.click('li.authority button.grant-authority');
  await stubbed.waitForSelector('#problem:not([hidden])');
  const afterWrongWord = await stubbed.evaluate(() => window.__decisions.length);

  await stubbed.fill('li.authority input.confirm', 'allow');
  await stubbed.click('li.authority button.grant-authority');
  await stubbed.waitForFunction(() => window.__decisions.length > 0);

  const decisions = await stubbed.evaluate(() => window.__decisions);

  assert.equal(afterWrongWord, 0, 'typing the wrong word decides nothing');
  assert.deepEqual(decisions, [{ capability: 'docker', allow: true }]);

  await stubbed.close();
});

await browser.close();
server.close();

console.log(`\n${failures.length === 0 ? 'All rendering checks passed.' : `${failures.length} failed.`}`);
process.exit(failures.length === 0 ? 0 : 1);
