// Screenshots of viewer states, taken in headless Chrome over the DevTools protocol.
//
//   node verify/screenshot.mjs out/shots/before shots.json
//
// shots.json is a list of { name, url, steps }, where each step is a JavaScript expression evaluated in
// the page after it has rendered (to click a node, press Esc, ...). Each shot is written as <name>.png.

import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';

const CHROME = process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const PORT = 9333;

const [outDir, shotsFile] = process.argv.slice(2);
const shots = JSON.parse(readFileSync(shotsFile, 'utf8'));

mkdirSync(outDir, { recursive: true });

const chrome = spawn(CHROME, ['--headless=new', `--remote-debugging-port=${PORT}`, '--user-data-dir=/tmp/puml-chrome', '--window-size=1280,800', 'about:blank'], { stdio: 'ignore' });

try {
    const page = await connect();

    await page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });

    for (const shot of shots) {
        await page.send('Page.navigate', { url: shot.url });
        await waitFor(page, `!!document.querySelector('#canvas svg') || !!document.querySelector('#canvas .message:not(:empty)')`);

        for (const step of shot.steps ?? []) {
            await evaluate(page, step);
            await sleep(300);
        }

        const { data } = await page.send('Page.captureScreenshot', { format: 'png' });

        writeFileSync(`${outDir}/${shot.name}.png`, Buffer.from(data, 'base64'));
        console.log(`${outDir}/${shot.name}.png`);
    }
} finally {
    chrome.kill();
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

    throw new Error('Chrome did not start');
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

async function evaluate(page, expression) {
    const { result, exceptionDetails } = await page.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });

    if (exceptionDetails) {
        throw new Error(`${expression}: ${exceptionDetails.exception?.description ?? exceptionDetails.text}`);
    }

    return result.value;
}

async function waitFor(page, condition) {
    for (let attempt = 0; attempt < 100; attempt++) {
        if (await evaluate(page, condition).catch(() => false)) {
            await sleep(200);

            return;
        }

        await sleep(100);
    }

    throw new Error(`timed out waiting for ${condition}`);
}
