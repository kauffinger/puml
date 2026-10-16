import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export const repository = fileURLToPath(new URL('..', import.meta.url));
export const extractor = `${repository}extractor/target/release/puml-extract`;

export function extract(projectDir) {
    if (!existsSync(extractor)) {
        throw new Error('Build the extractor first: (cd extractor && cargo build --release)');
    }

    return JSON.parse(execFileSync(extractor, [projectDir], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }));
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
