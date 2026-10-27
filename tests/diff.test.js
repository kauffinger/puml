import { strict as assert } from 'node:assert';
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { extract, extractor, git, readJson, repositoryWithPullRequest } from './support.js';

const expected = readJson('fixtures/blog-pr/expected/diff.json');

function diff(dir, base, ...flags) {
    const result = spawnSync(extractor, ['diff', dir, base, ...flags], { encoding: 'utf8' });

    return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

function diffJson(dir, base) {
    const result = diff(dir, base, '--format', 'json');
    assert.equal(result.status, 0, result.stderr);

    return JSON.parse(result.stdout);
}

const edge = (edge) => `${edge.from} -> ${edge.to} ${edge.kind} @${edge.evidence[0].file}:${edge.evidence[0].line}`;
const file = (file) => `${file.status} ${file.path}${file.from ? ` from ${file.from}` : ''}`;
const numbers = (numbers) =>
    numbers && { ...numbers, instability: numbers.instability === null ? null : Math.round(numbers.instability * 10000) / 10000 };
const memberChange = (member) =>
    member.change === 'changed' ? `changed ${member.before} -> ${member.after}` : `${member.change} ${member.before ?? member.after}`;

/** The diff in the shape of fixtures/blog-pr/expected/diff.json: everything but commits and timings. */
function summarize(diff, worsens) {
    return {
        files: diff.files.map(file),
        other_files: diff.other_files.map(file),
        classes: {
            added: diff.classes.added.map((added) => `${added.id} @${added.file}:${added.line}`),
            removed: diff.classes.removed.map((removed) => `${removed.id} @${removed.file}:${removed.line}`),
            renamed: diff.classes.renamed.map((renamed) => `${renamed.from.id} -> ${renamed.to.id}`),
            modified: Object.fromEntries(diff.classes.modified.map((modified) => [modified.class.id, modified.members.map(memberChange)])),
        },
        edges: { added: diff.edges.added.map(edge), removed: diff.edges.removed.map(edge) },
        modules: diff.modules.map((change) => `${change.from} -> ${change.to} ${change.before} -> ${change.after}, ${change.violations} violating`),
        violations: {
            added: diff.violations.added.map((violation) => `${violation.rule} ${edge(violation.edge)}`),
            removed: diff.violations.removed.map((violation) => `${violation.rule} ${edge(violation.edge)}`),
        },
        cycles: diff.cycles.map((cycle) => ({
            status: cycle.status,
            after: cycle.after,
            joined: cycle.joined,
            left: cycle.left,
            cut_class_edges: cycle.cost?.class_edges ?? null,
        })),
        metrics: Object.fromEntries(diff.metrics.map((change) => [change.module, { before: numbers(change.before), after: numbers(change.after) }])),
        outliers: { added: diff.outliers.added.map((outlier) => outlier.id), removed: diff.outliers.removed.map((outlier) => outlier.id) },
        dangling: diff.dangling.flatMap((dangling) =>
            dangling.used_by.map((use) => `${dangling.class} <- ${use.from} ${use.kind} @${use.evidence[0].file}:${use.evidence[0].line}`),
        ),
        impact: {
            classes: diff.impact.classes.map(
                (entry) => `${entry.class}${entry.api_changed ? ' api changed' : ''} <- ${entry.dependents.join(', ')}`,
            ),
            reach: diff.impact.reach,
        },
        reading_order: diff.reading_order,
        notes: diff.notes,
        worsens,
    };
}

const key = (edge) => [edge.from, edge.to, edge.kind, ['static', 'signature'].includes(edge.kind) ? '' : (edge.label ?? '')].join('|');

describe('a pull request on the blog', () => {
    let dir;

    before(() => {
        dir = repositoryWithPullRequest();
    });

    after(() => rmSync(dir, { recursive: true, force: true }));

    test('is described the same before it is committed', () => {
        const result = diffJson(dir, 'HEAD');

        assert.equal(result.head.uncommitted, true);
        assert.deepEqual(summarize(result, diff(dir, 'HEAD', '--strict').status === 1), expected);
    });

    test('and after it is committed on a branch', () => {
        git(dir, 'checkout', '--quiet', '-b', 'pull-request');
        git(dir, 'add', '--all');
        git(dir, '-c', 'user.name=puml', '-c', 'user.email=puml@example.com', 'commit', '--quiet', '-m', 'Publish posts from a PostData object');

        const result = diffJson(dir, 'main');

        assert.equal(result.head.uncommitted, undefined);
        assert.equal(result.base.commit, git(dir, 'rev-parse', 'main').trim());
        assert.deepEqual(summarize(result, diff(dir, 'main', '--strict').status === 1), expected);
    });

    test('adds and removes exactly the edges between a fresh extraction of each side', () => {
        const head = extract(dir);
        git(dir, 'checkout', '--quiet', 'main');
        const base = extract(dir);
        git(dir, 'checkout', '--quiet', 'pull-request');

        const [before, now] = [new Set(base.edges.map(key)), new Set(head.edges.map(key))];
        const result = diffJson(dir, 'main');

        assert.deepEqual(result.edges.added.map(key).sort(), [...now].filter((edge) => !before.has(edge)).sort());
        assert.deepEqual(result.edges.removed.map(key).sort(), [...before].filter((edge) => !now.has(edge)).sort());
    });

    test('has nothing to say about a branch compared with itself', () => {
        const result = diffJson(dir, 'HEAD');

        assert.deepEqual(result.files, []);
        assert.deepEqual(result.classes, { added: [], removed: [], renamed: [], modified: [] });
        assert.deepEqual(result.edges, { added: [], removed: [] });
        assert.deepEqual([result.modules, result.cycles, result.metrics, result.dangling, result.reading_order], [[], [], [], [], []]);
        assert.equal(diff(dir, 'HEAD', '--strict').status, 0);
    });

    test('is written as text and as Markdown', () => {
        const text = diff(dir, 'main').stdout;
        const markdown = diff(dir, 'main', '--format', 'markdown').stdout;

        assert.match(text, /READ IN THIS ORDER/);
        assert.match(text, /removed public static function fromRequest\(StorePostRequest \$request\): self/);
        assert.match(markdown, /^\| 🔴 1 new, 🟢 1 fixed \| 🔴 1 grew \| 🔴 1 \| 1 added, 3 modified, 1 renamed \|/m);
        assert.match(markdown, /🔴 \*\*Removed but still used\*\* `NotifyAuthor` by `PostPublished`/);
        assert.match(markdown, /### The classes it changes\n\n```mermaid\nflowchart LR/);
        assert.match(markdown, /<summary><b>Read in this order<\/b>: 5 classes/);
    });

    test('fails on a base it cannot find', () => {
        const result = diff(dir, 'no-such-branch');

        assert.equal(result.status, 2);
        assert.match(result.stderr, /fetch-depth: 0/);
    });
});

describe('the pull request comment', () => {
    let dir;
    let blog;

    before(() => {
        dir = repositoryWithPullRequest('backend');
        blog = join(dir, 'backend');
    });

    after(() => rmSync(dir, { recursive: true, force: true }));

    const markdown = (...flags) => diff(blog, 'main', '--format', 'markdown', ...flags).stdout;
    const linked = (commit, place) => {
        const [file, line] = place.split(':');

        return `[\`${place}\`](https://github.com/owner/app/blob/${commit}/backend/${file}#L${line})`;
    };

    test('links only the places in the base while the head has uncommitted changes', () => {
        const comment = markdown('--repository', 'https://github.com/owner/app/');
        const base = git(dir, 'rev-parse', 'main').trim();

        assert.ok(comment.includes(`no longer breaks \`core-independent-of-http\` · ${linked(base, 'app/Jobs/PublishPost.php:15')}`), comment);
        assert.ok(comment.includes(' breaks `core-independent-of-http` · `app/Data/PostData.php:15`'), comment);
    });

    test('links each place to the commit it was read from', () => {
        git(dir, 'checkout', '--quiet', '-b', 'pull-request');
        git(dir, 'add', '--all');
        git(dir, '-c', 'user.name=puml', '-c', 'user.email=puml@example.com', 'commit', '--quiet', '-m', 'Publish posts from a PostData object');

        const comment = markdown('--repository', 'https://github.com/owner/app');
        const [base, head] = [git(dir, 'rev-parse', 'main').trim(), git(dir, 'rev-parse', 'HEAD').trim()];

        assert.ok(comment.includes(` breaks \`core-independent-of-http\` · ${linked(head, 'app/Data/PostData.php:15')}`), comment);
        assert.ok(comment.includes(`no longer breaks \`core-independent-of-http\` · ${linked(base, 'app/Jobs/PublishPost.php:15')}`), comment);
        assert.ok(comment.includes(`by \`PostPublished\` · ${linked(head, 'app/Events/PostPublished.php:16')}`), comment);
        assert.ok(comment.includes(`1. \`App\\Data\\PostData\` added · ${linked(head, 'app/Data/PostData.php:7')}`), comment);
        assert.doesNotMatch(markdown(), /\]\(https:/);
    });

    test('is one line and a fold when nothing in the verdict changed', () => {
        git(dir, 'checkout', '--quiet', '-b', 'quiet', 'main');
        const post = join(blog, 'app/Models/Post.php');
        writeFileSync(post, readFileSync(post, 'utf8').replace('public function url(): string', 'public function url(bool $absolute = true): string'));
        git(dir, '-c', 'user.name=puml', '-c', 'user.email=puml@example.com', 'commit', '--quiet', '--all', '-m', 'Absolute URLs');

        const comment = markdown();
        const [top] = comment.split('<details>');

        assert.match(top, /^## puml: ✅ nothing in the architecture got worse\n\n`main` \(\w{7}\) → `HEAD` \(\w{7}\)\n\n$/);
        assert.match(comment, /^<details><summary><b>Classes<\/b>: 1 modified · <b>used by<\/b> 6 directly, 8 in all<\/summary>$/m);
        assert.match(comment, /```mermaid\nflowchart LR/);
        assert.ok(comment.trimEnd().endsWith('</details>'));
    });

    test('folds a class graph above 40 classes and draws module edges above 100', () => {
        mkdirSync(join(blog, 'app/Probes'));

        const addProbes = (count) => {
            for (let number = 1; number <= count; number++) {
                writeFileSync(
                    join(blog, `app/Probes/Probe${number}.php`),
                    `<?php\n\nnamespace App\\Probes;\n\nclass Probe${number}\n{\n    public function post(): \\App\\Models\\Post\n    {\n    }\n}\n`,
                );
            }
        };

        addProbes(45);
        let comment = markdown();
        assert.match(comment, /^<details><summary><b>The classes it changes<\/b>: 46 classes<\/summary>\n\n```mermaid$/m);
        assert.doesNotMatch(comment, /### The classes it changes/);

        addProbes(100);
        comment = markdown();
        assert.match(comment, /### The module edges it changes/);
        assert.doesNotMatch(comment, /The classes it changes/);
    });
});
