// Runs reviewer tasks against the served viewer in headless Chrome over the DevTools protocol.
//
//   node verify/drive.mjs out/shots/before verify/tasks.json [base url]
//
// Each task is { name, url, steps, expect }. The steps are ui.* calls (see interact.js), one per
// interaction. Every string in expect (compared without case or whitespace) must have been shown (crumbs, SVG labels, side panel) at some
// point along the path to count as answered, and shown legibly without scrolling (on-screen.js) to
// count as readable. Answers still unread after the last step are looked for by paging the side
// panel down, one page per interaction, counted in scrolls. Then the page is measured (measure.js)
// and a screenshot is written as <name>.png. The results go to <outDir>/results.json.

import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';

const CHROME = process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const PORT = 9334;
const PAGE_DOWN = `(() => {
    const panel = document.getElementById('panel');
    const before = panel.scrollTop;
    panel.scrollTop += panel.clientHeight - 40;
    return panel.scrollTop > before;
})()`;

const [outDir, tasksFile, base = 'http://127.0.0.1:8765/out/'] = process.argv.slice(2);
const tasks = JSON.parse(readFileSync(tasksFile, 'utf8'));
const interact = readFileSync(new URL('./interact.js', import.meta.url), 'utf8');
const measure = readFileSync(new URL('./measure.js', import.meta.url), 'utf8');
const onScreen = readFileSync(new URL('./on-screen.js', import.meta.url), 'utf8');

mkdirSync(outDir, { recursive: true });

const chrome = spawn(CHROME, ['--headless=new', `--remote-debugging-port=${PORT}`, '--user-data-dir=/tmp/puml-chrome-drive', '--window-size=1280,800', 'about:blank'], { stdio: 'ignore' });
const results = [];
let page;

try {
    page = await connect();

    await page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
    await page.send('Network.setCacheDisabled', { cacheDisabled: true });

    for (const task of tasks) {
        await page.send('Page.navigate', { url: 'about:blank' });
        await page.send('Page.navigate', { url: new URL(task.url, base).href });
        await waitFor(page, `!!window.puml && !!document.querySelector('#canvas svg')`);
        await evaluate(page, interact);

        let error = null;
        let seen = await evaluate(page, 'ui.visible()');
        let read = await evaluate(page, onScreen);

        for (const step of task.steps ?? []) {
            try {
                await evaluate(page, step);
                seen += `\n${await evaluate(page, 'ui.visible()')}`;
                read += `\n${await evaluate(page, onScreen)}`;
            } catch (failure) {
                error = failure.message;
                break;
            }
        }

        const unreadIn = (text) => (task.expect ?? []).filter((expected) => !normalize(text).includes(normalize(expected)));
        let scrolls = 0;

        while (!error && unreadIn(read).length > 0 && scrolls < 10 && (await evaluate(page, PAGE_DOWN))) {
            scrolls++;
            read += `\n${await evaluate(page, onScreen)}`;
        }

        await sleep(200);

        const metrics = await evaluate(page, measure);

        if (process.env.DUMP) {
            console.log(await evaluate(page, 'ui.visible()'));
        }

        const missing = (task.expect ?? []).filter((text) => !normalize(seen).includes(normalize(text)));
        const unread = unreadIn(read);
        const { data } = await page.send('Page.captureScreenshot', { format: 'png' });

        writeFileSync(`${outDir}/${task.name}.png`, Buffer.from(data, 'base64'));

        const result = { name: task.name, interactions: (task.steps ?? []).length + scrolls, scrolls, answered: !error && missing.length === 0, missing, readable: !error && unread.length === 0, unread, error, ...metrics };

        results.push(result);
        console.log(JSON.stringify(result));
    }
} finally {
    await page?.send('Browser.close').catch(() => {});
    chrome.kill();
    writeFileSync(`${outDir}/results.json`, `${JSON.stringify(results, null, 2)}\n`);
    page?.close();
}

function normalize(text) {
    return text.toLowerCase().replace(/\s+/g, '');
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
        close: () => socket.close(),
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
        throw new Error(exceptionDetails.exception?.description?.split('\n')[0] ?? exceptionDetails.text);
    }

    return result.value;
}

async function waitFor(page, condition) {
    for (let attempt = 0; attempt < 150; attempt++) {
        if (await evaluate(page, condition).catch(() => false)) {
            await sleep(200);

            return;
        }

        await sleep(100);
    }

    throw new Error(`timed out waiting for ${condition}`);
}
