// Films somebody using the window, without anybody watching it.
//
// "A UI no human has seen has problems no test finds" is true, and a test that
// asserts `textContent` does not fix it — somebody has to *look*. This drives
// the real frontend through a real interaction and records it, so looking is
// possible on a machine with no display.
//
// It is not a test and does not assert. It produces a film and a set of
// labelled stills, and then a person reads them. The three defects in the
// commit that introduced this file were all found that way, by looking at
// frames, while every one of the rendering tests passed.
//
// What this is: the actual `ui/` modules, in a real browser, driven by real
// clicks and keystrokes, against view data shaped exactly as `ephemeral-api`
// serialises it.
//
// What it is not: the Tauri window itself. That is `film-window.sh`, which runs
// the real binary under a virtual display. Chromium is not WebKit, and this
// film shows the page rather than the window around it.
//
//   node tests/film.mjs
//   CHROMIUM_PATH=/path/to/chromium node tests/film.mjs

import { chromium } from 'playwright';
import { createServer } from 'node:http';
import { readFile, mkdir, rm, rename, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join, extname } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const ui = join(here, '..', 'ui');
const out = join(here, '..', 'recordings');

// A stale frame from a previous take is worse than no frame: it is a picture of
// a bug somebody already fixed, and it looks exactly as current as the rest.
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

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

// Shaped exactly as ephemeral-api serialises these. A film of invented shapes
// would be a film of something that does not exist.
const applications = [
  {
    id: 'apartment-comparator-3f2a1b9c',
    name: 'Apartment Comparator',
    purpose: 'compare these two CSV files and show me what changed',
    state: 'Ready',
    state_kind: 'idle',
    runnable: true,
    running: false,
    put_away: false,
    granted: 0,
    highest_granted_risk: null,
    awaiting_decision: 2,
    updated_at: '2026-08-15T20:00:00Z',
  },
  {
    id: 'word-counter-8b1d0a42',
    name: 'Word Counter',
    purpose: 'count the words in a text file',
    state: 'Running',
    state_kind: 'active',
    runnable: true,
    running: true,
    put_away: false,
    granted: 1,
    highest_granted_risk: 'medium',
    awaiting_decision: 0,
    updated_at: '2026-08-15T19:40:00Z',
  },
];

// One ordinary request and one that is as wide as Ephemeral gets, because the
// difference between them is the thing this window exists to make visible.
const requests = [
  {
    capability: 'filesystem_read',
    wants: 'read the files in ~/Downloads/apartments',
    reason: 'to compare the two CSV files you selected',
    if_allowed:
      'It can read what is at ~/Downloads/apartments. It cannot change those files, and it cannot see anything else on this device.',
    risk: 'medium',
    needs_explicit_confirmation: false,
    revocable: true,
  },
  {
    capability: 'network_outbound',
    wants: 'connect to anywhere on the internet',
    reason: 'to look up current market prices',
    if_allowed:
      'It can send anything it can read to anywhere. This is the widest permission Ephemeral offers.',
    risk: 'critical',
    needs_explicit_confirmation: true,
    revocable: true,
  },
];

const base = {
  explanation: 'The app is built and ready. It is not running and uses nothing.',
  description: 'Ready',
  runtime: {
    kind: 'docker',
    isolation:
      'This app runs in a container on this device. It can only reach what you have allowed it to reach.',
    runs_locally: true,
    image: 'python:3.12-slim',
    interface: 'command_line',
    primary_action: 'Run',
  },
  limits: {
    description:
      'up to 0.50 CPU cores, 512 MiB of memory, 1024 MiB of disk, stops after 15 minutes',
    cpu_millis: 500,
    memory_mib: 512,
    storage_mib: 1024,
  },
  // Two versions and a third that predates snapshots, because what a person
  // needs to see here is the difference between a version they can go back to
  // and one that is only recorded. A film with an empty history would show none
  // of the page that offers a rollback.
  versions: [
    {
      digest: 'c4f1d90a8b21',
      sequence: 3,
      reason: 'repaired',
      created_at: '2026-08-20T09:12:00Z',
      current: true,
      source_kept: true,
    },
    {
      digest: '7b3e5a2c1d40',
      sequence: 2,
      reason: 'generated',
      created_at: '2026-08-19T17:40:00Z',
      current: false,
      source_kept: true,
    },
    {
      digest: '10ab99ce7f22',
      sequence: 1,
      reason: 'generated',
      created_at: '2026-08-18T11:05:00Z',
      current: false,
      source_kept: false,
    },
  ],
  retention: 'keep for 1 week',
};

const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
const size = { width: 900, height: 820 };
const context = await browser.newContext({
  viewport: size,
  deviceScaleFactor: 2,
  recordVideo: { dir: out, size },
});
const page = await context.newPage();

// Stands in for Tauri's bridge. The frontend cannot tell the difference, which
// is the point: this is the same code path the real window takes, including the
// reload after a decision.
//
// Every bit of state lives in the init script so it survives that reload. State
// set with `evaluate` does not, which is how the first take of this film ended
// up clicking a permission that had never rendered.
await page.addInitScript(
  ({ apps, asked, detail }) => {
    window.__FILM__ = { allowed: [], denied: [] };

    const answered = () => [...window.__FILM__.allowed, ...window.__FILM__.denied];

    // The same ordering ephemeral-core gives RiskLevel, because a film that
    // reported a lower risk than the real service would be a film of a window
    // that does not exist — and the first take did exactly that, drawing an app
    // holding the whole internet in the same green as one holding nothing.
    const ORDER = ['low', 'medium', 'high', 'critical'];
    const highest = (permissions) =>
      permissions.length
        ? permissions.map((p) => p.risk).sort((a, b) => ORDER.indexOf(a) - ORDER.indexOf(b)).pop()
        : null;

    const view = (id) => {
      const outstanding = asked.filter((r) => !answered().includes(r.capability));
      const allowed = asked.filter((r) => window.__FILM__.allowed.includes(r.capability));
      const summary = apps.find((a) => a.id === id) ?? apps[0];

      return {
        ...detail,
        summary: {
          ...summary,
          awaiting_decision: outstanding.length,
          granted: allowed.length,
          highest_granted_risk: highest(allowed),
        },
        permissions: {
          allowed,
          outstanding,
          highest_granted_risk: highest(allowed),
          isolated: allowed.length === 0,
        },
      };
    };

    window.__TAURI__ = {
      core: {
        invoke: async (command, args) => {
          if (command === 'applications') {
            return apps.map((app) =>
              app.id.startsWith('apartment')
                ? { ...app, ...view(app.id).summary }
                : app,
            );
          }
          if (command === 'application') return view(args.id);
          if (command === 'decide') {
            const into = args.allow ? window.__FILM__.allowed : window.__FILM__.denied;
            if (!into.includes(args.capability)) into.push(args.capability);
            return null;
          }
          if (command === 'rollback') {
            return {
              app: args.id,
              sequence: 2,
              digest: args.version,
              grants_withdrawn: 1,
              newly_requested: 1,
              headline: `Rolled ${args.id} back to version 2 (${args.version}).`,
              caution:
                'This version asks for 1 thing(s) the one it replaced had stopped needing, so ' +
                '1 grant(s) were withdrawn. Look at what it asks for now and allow again only ' +
                'what you still want.',
              note:
                'The built image was cleared: a version is its source, and running the newer ' +
                "build under this version's name would report one thing and run another. " +
                'Generate again to rebuild.',
            };
          }
          if (command === 'api_version') return 2;
          throw new Error(`no such command: ${command}`);
        },
      },
    };
  },
  { apps: applications, asked: requests, detail: base },
);

// Each still is named for what a person is meant to check in it, so a reviewer
// reading the directory knows what they are looking for before they open it.
let taken = 0;
const scenes = [];
async function scene(what, pause = 1500) {
  await page.waitForTimeout(pause);
  taken += 1;
  const name = `${String(taken).padStart(2, '0')}-${what}.png`;
  await page.screenshot({ path: join(out, name) });
  scenes.push(name);
}

await page.goto(origin);

// The list. An app waiting on a decision must be the thing that catches an eye.
await scene('the-list', 1800);

await page.click('li.application');
await scene('what-it-is-asking-for', 1600);

// An ordinary request: one click. The page must stay put and say what changed —
// answering something used to throw you back to the list, so the page
// confirming what you had allowed was never seen.
await page.click('li.permission[data-capability="filesystem_read"] button.allow');
await scene('what-you-have-allowed', 2000);

// A critical request refuses a click. Typing the wrong word must also refuse.
const critical = 'li.permission[data-capability="network_outbound"]';
await page.click(`${critical} input.confirm`);
await page.type(`${critical} input.confirm`, 'yes', { delay: 200 });
await page.click(`${critical} button.confirm-allow`);
await scene('the-wrong-word-grants-nothing', 1800);

// And the right one is accepted — the path that had no button at all until
// somebody looked at a frame of this film.
await page.fill(`${critical} input.confirm`, '');
await page.type(`${critical} input.confirm`, 'allow', { delay: 200 });
await page.click(`${critical} button.confirm-allow`);
await scene('the-word-typed-in-full', 2000);

// What it has been. A version whose source is gone must read as an absence
// rather than as an offer somebody's click will bounce off.
await page.evaluate(() => document.querySelector('section.versions')?.scrollIntoView());
await scene('what-it-has-been', 1500);

// Rolling back asks twice, and the first click has to say what the second one
// costs — the build and, possibly, permissions already granted.
await page.click('li.version:not(.current) button.rollback');
await scene('what-going-back-costs', 1600);

// And what it says afterwards, which is the sentence somebody has to act on.
await page.click('button.confirm-rollback');
await scene('what-the-rollback-took-back', 2000);

// Back out: with everything answered, nothing should still be asking.
await page.click('button.back');
await scene('nothing-left-waiting', 1800);

await context.close();

// Playwright names the video after the page, which tells a reader nothing.
const [video] = (await readdir(out)).filter((name) => name.endsWith('.webm'));
if (video && video !== 'walkthrough.webm') {
  await rename(join(out, video), join(out, 'walkthrough.webm'));
}

await browser.close();
server.close();

console.log(`\nFilmed into ${out}`);
console.log('  walkthrough.webm');
for (const name of scenes) console.log(`  ${name}`);
console.log('\nNow look at them. That is the whole point.');
