import { strict as assert } from 'node:assert';
import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const repository = fileURLToPath(new URL('..', import.meta.url));
export const extractor = `${repository}extractor/target/release/puml-extract`;

export function extract(projectDir) {
    if (!existsSync(extractor)) {
        throw new Error('Build the extractor first: (cd extractor && cargo build --release)');
    }

    return JSON.parse(execFileSync(extractor, [projectDir], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }));
}

export function git(dir, ...args) {
    const result = spawnSync('git', ['-C', dir, ...args], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);

    return result.stdout;
}

/** The blog fixture committed once, with the pull request in fixtures/blog-pr laid over it in the working tree. */
export function repositoryWithPullRequest() {
    const dir = mkdtempSync(join(tmpdir(), 'puml-diff-'));
    cpSync(`${repository}fixtures/blog`, dir, { recursive: true, filter: (path) => !path.includes('/expected') });

    git(dir, 'init', '--quiet', '--initial-branch=main');
    git(dir, 'add', '--all');
    git(dir, '-c', 'user.name=puml', '-c', 'user.email=puml@example.com', 'commit', '--quiet', '-m', 'Base');

    for (const path of readFileSync(`${repository}fixtures/blog-pr/deleted`, 'utf8').split('\n').filter(Boolean)) {
        rmSync(join(dir, path));
    }

    cpSync(`${repository}fixtures/blog-pr/head`, dir, { recursive: true });

    return dir;
}

export function readJson(path) {
    return JSON.parse(readFileSync(`${repository}${path}`, 'utf8'));
}

export function describeNodes(projection) {
    return projection.nodes.map((node) => (node.kind === 'other' ? `other ${node.label}` : `${node.kind} ${node.id}`)).sort();
}

export function describeEdges(projection) {
    return projection.edges.map((edge) => `${edge.from} -> ${edge.to} ${edge.weight}${edge.cycle ? ' cycle' : ''}`).sort();
}
