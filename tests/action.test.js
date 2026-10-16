import { strict as assert } from 'node:assert';
import { spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { extractor, git, repository, repositoryWithPullRequest } from './support.js';

/** Stands in for the GitHub CLI: logs each call, answers --jq from a file of comments, keeps the posted body. */
const fakeGh = `#!/usr/bin/env bash
echo "$*" >> "$GH_LOG"
previous=
for argument in "$@"; do
    if [ "$previous" = --jq ]; then jq -r "$argument" "$GH_COMMENTS"; fi
    case "$argument" in body=@*) cp "\${argument#body=@}" "$GH_BODY" ;; esac
    previous=$argument
done
`;

describe('the GitHub Action', () => {
    let dir;
    let runner;

    before(() => {
        dir = repositoryWithPullRequest();
        git(dir, 'update-ref', 'refs/remotes/origin/main', 'main');
        runner = mkdtempSync(join(tmpdir(), 'puml-action-'));
        mkdirSync(join(runner, 'bin'));
        writeFileSync(join(runner, 'bin', 'gh'), fakeGh);
        chmodSync(join(runner, 'bin', 'gh'), 0o755);
    });

    after(() => {
        rmSync(dir, { recursive: true, force: true });
        rmSync(runner, { recursive: true, force: true });
    });

    function report({ base = '', baseBranch = 'main', strict = 'false', comments = [] } = {}) {
        const run = mkdtempSync(join(runner, 'run-'));
        const file = (name) => join(run, name);
        writeFileSync(file('comments.json'), JSON.stringify(comments));
        writeFileSync(file('output'), '');
        writeFileSync(file('summary'), '');

        const result = spawnSync(`${repository}action/report.sh`, {
            encoding: 'utf8',
            env: {
                PATH: `${join(runner, 'bin')}:${dirname(extractor)}:${process.env.PATH}`,
                RUNNER_TEMP: run,
                GITHUB_OUTPUT: file('output'),
                GITHUB_STEP_SUMMARY: file('summary'),
                GITHUB_REPOSITORY: 'owner/app',
                GITHUB_BASE_REF: baseBranch,
                GH_LOG: file('gh.log'),
                GH_COMMENTS: file('comments.json'),
                GH_BODY: file('body.md'),
                PULL_REQUEST: '12',
                PUML_BASE: base,
                PUML_DIRECTORY: dir,
                PUML_COMMENT: 'true',
                PUML_STRICT: strict,
            },
        });
        const read = (name) => (existsSync(file(name)) ? readFileSync(file(name), 'utf8') : '');

        return {
            status: result.status,
            stdout: result.stdout,
            stderr: result.stderr,
            output: read('output'),
            summary: read('summary'),
            calls: read('gh.log').split('\n').filter(Boolean),
            body: read('body.md'),
        };
    }

    test('comments on a pull request that changes classes', () => {
        const result = report();

        assert.equal(result.status, 0, result.stderr);
        assert.match(result.output, /^worsens=true$/m);
        assert.match(result.summary, /### Read in this order/);
        assert.equal(result.calls.length, 2);
        assert.match(result.calls[1], /^api --method POST repos\/owner\/app\/issues\/12\/comments /);
        assert.ok(result.body.startsWith('<!-- puml-diff -->\n## puml:'));
        assert.match(result.body, /^`origin\/main` \(\w{7}\) → `HEAD`/m);
    });

    test('updates its own comment instead of adding another', () => {
        const result = report({
            comments: [
                { id: 3, body: 'Looks good to me' },
                { id: 7, body: '<!-- puml-diff -->\n## puml: an older report' },
            ],
        });

        assert.equal(result.status, 0, result.stderr);
        assert.match(result.calls[1], /^api --method PATCH repos\/owner\/app\/issues\/comments\/7 /);
    });

    test('fails a strict run after commenting', () => {
        const result = report({ strict: 'true' });

        assert.equal(result.status, 1);
        assert.match(result.stdout, /::error::This pull request makes the architecture worse/);
        assert.match(result.calls[1], /--method POST/);
    });

    test('stops on a base it cannot find', () => {
        const result = report({ base: 'no-such-branch' });

        assert.equal(result.status, 2);
        assert.match(result.stderr, /fetch-depth: 0/);
        assert.deepEqual(result.calls, []);
    });

    test('needs a base outside a pull request', () => {
        const result = report({ baseBranch: '' });

        assert.equal(result.status, 1);
        assert.match(result.stdout, /::error::Set the base input/);
        assert.deepEqual(result.calls, []);
    });

    test('stays quiet on a pull request that changes no classes', () => {
        git(dir, 'add', '--all');
        git(dir, '-c', 'user.name=puml', '-c', 'user.email=puml@example.com', 'commit', '--quiet', '-m', 'Pull request');

        const result = report({ base: 'HEAD' });

        assert.equal(result.status, 0, result.stderr);
        assert.match(result.output, /^worsens=false$/m);
        assert.equal(result.calls.length, 1);
        assert.match(result.calls[0], /^api --paginate /);
    });
});
