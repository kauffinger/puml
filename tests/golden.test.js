import assert from 'node:assert/strict';
import { test } from 'node:test';

import { formatMethod, formatProperty, indexGraph, project } from '../viewer/model.js';
import { describeEdges, describeNodes, extract, readJson, repository } from './support.js';

const graph = extract(`${repository}fixtures/shop`);
const expected = readJson('fixtures/shop/expected/graph.json');
const levels = readJson('fixtures/shop/expected/levels.json');

test('the graph declares its format version', () => {
    assert.equal(graph.version, 1);
    assert.equal(graph.project, 'fixture/shop');
    assert.deepEqual(graph.roots, ['Shop']);
});

test('every class and external class is found with its package and kind', () => {
    const actual = Object.fromEntries(
        graph.classes.map((cls) => {
            const described = { package: cls.package, external: cls.external };

            if (!cls.external) {
                described.kind = cls.kind;
            }

            for (const key of ['parent', 'interfaces', 'traits', 'cases']) {
                if (cls[key] !== undefined) {
                    described[key] = cls[key];
                }
            }

            return [cls.id, described];
        }),
    );

    const wanted = Object.fromEntries(
        Object.entries(expected.classes).map(([id, cls]) => [id, { ...cls }]),
    );

    assert.deepEqual(sortKeys(actual), sortKeys(wanted));
});

test('every edge is found with its kind, label and file:line evidence', () => {
    const actual = graph.edges.flatMap((edge) =>
        edge.evidence.map((evidence) => {
            const label = edge.label ? ` [${edge.label}]` : '';

            return `${edge.from} -> ${edge.to} ${edge.kind}${label} @${evidence.file}:${evidence.line}`;
        }),
    );

    assert.deepEqual(actual.sort(), [...expected.edges].sort());
});

test('eloquent relations are recorded on the models that declare them', () => {
    const actual = Object.fromEntries(
        graph.classes
            .filter((cls) => cls.relations)
            .map((cls) => [cls.id, cls.relations.map((relation) => `${relation.name} ${relation.type} ${relation.related ?? '?'} @${relation.line}`)]),
    );

    assert.deepEqual(sortKeys(actual), sortKeys(expected.relations));
});

test('members carry visibility and resolved types', () => {
    for (const [id, members] of Object.entries(expected.members)) {
        const cls = graph.classes.find((candidate) => candidate.id === id);

        assert.deepEqual((cls.properties ?? []).map(formatProperty), members.properties, `${id} properties`);
        assert.deepEqual((cls.methods ?? []).map(formatMethod), members.methods, `${id} methods`);
    }
});

test('what could not be resolved is reported', () => {
    const actual = graph.unresolved.map((note) => `${note.kind} @${note.file}:${note.line}`);

    assert.deepEqual(actual.sort(), [...expected.unresolved].sort());
});

for (const [name, golden] of Object.entries(levels)) {
    test(`level golden: ${name}`, () => {
        const projection = project(indexGraph(graph), golden.view);

        assert.deepEqual(describeNodes(projection), [...golden.nodes].sort());
        assert.deepEqual(describeEdges(projection), [...golden.edges].sort());
    });
}

function sortKeys(object) {
    return Object.fromEntries(Object.entries(object).sort(([a], [b]) => a.localeCompare(b)));
}
