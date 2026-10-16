// A persistent headless Chrome a reviewer (or the judge) drives one interaction at a time.
//
//   node verify/browse.mjs start                 start Chrome (once; PUML_BROWSE_PORT picks the port)
//   node verify/browse.mjs open <url>            open a page, then a screenshot
//   node verify/browse.mjs click <label>         click the node whose label is <label>
//   node verify/browse.mjs dblclick <label>      double-click a node
//   node verify/browse.mjs click-edge <from> <to>
//   node verify/browse.mjs dblclick-edge <from> <to>
//   node verify/browse.mjs link <text>           click a link or toggle in the side panel or breadcrumb
//   node verify/browse.mjs search <name>         type a name into the search box and press Enter
//   node verify/browse.mjs key <key>             press a key, e.g. Escape
//   node verify/browse.mjs panel                 print the side panel's text (as if scrolled through)
//   node verify/browse.mjs shot                  only take a screenshot
//   node verify/browse.mjs stop
//
// Every command except panel and stop writes a 1280x800 screenshot and prints its path. Set
// PUML_BROWSE_DIR to choose where (default /tmp/puml-browse).

import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';

const CHROME = process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const PORT = Number(process.env.PUML_BROWSE_PORT ?? 9335);
const DIR = process.env.PUML_BROWSE_DIR ?? '/tmp/puml-browse';
const interact = readFileSync(new URL('./interact.js', import.meta.url), 'utf8');

const [command, ...args] = process.argv.slice(2);

mkdirSync(DIR, { recursive: true });

if (command === 'start') {
    spawn(CHROME, ['--headless=new', `--remote-debugging-port=${PORT}`, `--user-data-dir=/tmp/puml-browse-chrome-${PORT}`, '--window-size=1280,800', 'about:blank'], {
        stdio: 'ignore',
        detached: true,
    }).unref();
    await connect();
    console.log(`Chrome is listening on ${PORT}`);
    process.exit(0);
}

const page = await connect();

if (command === 'stop') {
    await page.send('Browser.close').catch(() => {});
    process.exit(0);
}

await page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });

const quote = JSON.stringify;
const actions = {
    open: async ([url]) => {
        await page.send('Page.navigate', { url: 'about:blank' });
        await page.send('Page.navigate', { url });
        await waitFor(`!!window.puml && !!document.querySelector('#canvas svg')`);
    },
    click: ([label]) => run(`ui.click(${quote(label)})`),
    dblclick: ([label]) => run(`ui.dblclick(${quote(label)})`),
    'click-edge': ([from, to]) => run(`ui.clickEdge(${quote(from)}, ${quote(to)})`),
    'dblclick-edge': ([from, to]) => run(`ui.dblclickEdge(${quote(from)}, ${quote(to)})`),
    link: ([text]) => run(`ui.link(${quote(text)})`),
    search: ([name]) => run(`ui.search(${quote(name)})`),
    key: ([key]) => run(`ui.key(${quote(key)})`),
    shot: async () => {},
    panel: async () => {
        console.log(await evaluate(`document.getElementById('panel').innerText`));
        process.exit(0);
    },
};

if (!actions[command]) {
    console.error(`unknown command ${command}`);
    process.exit(1);
}

try {
    await actions[command](args);
} catch (error) {
    console.error(error.message);
}

await sleep(300);

const { data } = await page.send('Page.captureScreenshot', { format: 'png' });
const file = `${DIR}/${Date.now()}-${command}.png`;

writeFileSync(file, Buffer.from(data, 'base64'));
console.log(file);
process.exit(0);

async function run(expression) {
    if (!(await evaluate('!!window.ui'))) {
        await evaluate(interact);
    }

    await evaluate(expression);
}

async function connect() {
    for (let attempt = 0; attempt < 50; attempt++) {
        try {
            const targets = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
            const target = targets.find((candidate) => candidate.type === 'page');

            if (target) {
                return session(target.webSocketDebuggerUrl);
            }
        } catch {}

        await sleep(200);
    }

    throw new Error(`no Chrome on port ${PORT}; run: node verify/browse.mjs start`);
}

function session(url) {
    const socket = new WebSocket(url);
    const pending = new Map();
    let id = 0;

    socket.addEventListener('message', (event) => {
        const message = JSON.parse(event.data);

        if (pending.has(message.id)) {
            const { resolve, reject } = pending.get(message.id);
            pending.delete(message.id);
            message.error ? reject(new Error(message.error.message)) : resolve(message.result);
        }
    });

    const opened = new Promise((resolve) => socket.addEventListener('open', resolve));

    return {
        async send(method, params = {}) {
            await opened;

            return new Promise((resolve, reject) => {
                pending.set(++id, { resolve, reject });
                socket.send(JSON.stringify({ id, method, params }));
            });
        },
    };
}

async function evaluate(expression) {
    const { result, exceptionDetails } = await page.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });

    if (exceptionDetails) {
        throw new Error(exceptionDetails.exception?.description?.split('\n')[0] ?? exceptionDetails.text);
    }

    return result.value;
}

async function waitFor(condition) {
    for (let attempt = 0; attempt < 150; attempt++) {
        if (await evaluate(condition).catch(() => false)) {
            await sleep(200);

            return;
        }

        await sleep(100);
    }

    throw new Error(`timed out waiting for the page`);
}
