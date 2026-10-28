import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { formatMethod, formatProperty, indexGraph, project } from '../viewer/model.js';
import { describeEdges, describeNodes, extract, readJson, repository } from './support.js';

const graph = extract(`${repository}fixtures/shop`);
const expected = readJson('fixtures/shop/expected/graph.json');
const levels = readJson('fixtures/shop/expected/levels.json');

test('the graph declares its format version', () => {
    assert.equal(graph.version, 3);
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

test('the methods every enum has are written as PHP declares them, in the same order on every run', () => {
    const dir = mkdtempSync(join(tmpdir(), 'puml-enums-'));
    mkdirSync(join(dir, 'src'));
    writeFileSync(join(dir, 'composer.json'), JSON.stringify({ name: 'fixture/enums', autoload: { 'psr-4': { 'App\\': 'src/' } } }));
    writeFileSync(join(dir, 'src/Status.php'), "<?php\n\nnamespace App;\n\nenum Status: string\n{\n    case Paid = 'paid';\n}\n");
    writeFileSync(join(dir, 'src/Suit.php'), '<?php\n\nnamespace App;\n\nenum Suit\n{\n    case Hearts;\n}\n');

    const signatures = () =>
        Object.fromEntries(
            extract(dir)
                .classes.filter((cls) => !cls.external)
                .map((cls) => [cls.id, cls.methods.map((method) => formatMethod(method))]),
        );

    try {
        const first = signatures();

        assert.deepEqual(first, {
            'App\\Status': ['+cases(): array', '+from(value: string|int): static', '+tryFrom(value: string|int): ?static'],
            'App\\Suit': ['+cases(): array'],
        });

        for (let run = 0; run < 5; run++) {
            assert.deepEqual(signatures(), first);
        }
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
});
