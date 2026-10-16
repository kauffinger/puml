import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { test } from 'node:test';

import { DEFAULT_BUDGETS, indexGraph, project } from '../viewer/model.js';
import { extract, readJson, repository } from './support.js';

const blog = indexGraph(extract(`${repository}fixtures/blog`));
const shop = indexGraph(extract(`${repository}fixtures/shop`));
const expected = readJson('fixtures/blog/expected/violations.json');

const violations = (index) => index.graph.verdict.violations.map(({ rule, edge }) => ({ rule, edge: index.graph.edges[edge] }));
const moduleCycles = (index) => index.graph.verdict.cycles;
const moduleMetrics = (index) => index.graph.verdict.modules;

const describe = ({ rule, edge }) => edge.evidence.map((place) => `${rule} ${edge.from} -> ${edge.to} ${edge.kind} @${place.file}:${place.line}`);

test('blog golden: every seeded violation is found at its line, and nothing else', () => {
    assert.deepEqual(violations(blog).flatMap(describe), expected.violations);
});

test('blog golden: the seeded module cycle', () => {
    assert.deepEqual(moduleCycles(blog).map((cycle) => cycle.modules), expected.cycles);
});

test('blog golden: hand-computed coupling metrics', () => {
    const metrics = new Map(moduleMetrics(blog).map((module) => [module.id, module]));

    for (const [id, wanted] of Object.entries(expected.metrics)) {
        const actual = metrics.get(id);

        assert.ok(actual, id);

        for (const [key, value] of Object.entries(wanted)) {
            if (typeof value === 'number') {
                assert.ok(Math.abs(actual[key] - value) < 0.001, `${id} ${key}: ${actual[key]} !== ${value}`);
            } else {
                assert.equal(actual[key], value, `${id} ${key}`);
            }
        }
    }
});

test('blog golden: every module lands in its layer', () => {
    const actual = Object.fromEntries(blog.graph.verdict.layers.map((entry) => [entry.namespace, entry.layer]));

    assert.deepEqual(actual, expected.layers);
});

test('the shop fixture breaks no rule but has its billing/orders cycle', () => {
    assert.deepEqual(violations(shop), []);
    assert.deepEqual(moduleCycles(shop).map((cycle) => cycle.modules), [['Shop\\Billing', 'Shop\\Orders']]);
    assert.deepEqual(moduleCycles(shop)[0].cut.map((edge) => `${edge.from} -> ${edge.to}`), ['Shop\\Billing -> Shop\\Orders']);
});

test('the cheapest cut really breaks the cycle', () => {
    for (const index of [blog, shop]) {
        for (const cycle of moduleCycles(index)) {
            const cut = new Set(cycle.cut.map((edge) => `${edge.from} -> ${edge.to}`));
            const remaining = cycle.edges.filter((edge) => !cut.has(`${edge.from} -> ${edge.to}`));
            const projection = { nodes: cycle.modules, edges: remaining };

            assert.ok(acyclic(projection), cycle.modules.join(', '));
        }
    }
});

function acyclic({ nodes, edges }) {
    const successors = new Map(nodes.map((id) => [id, []]));

    for (const edge of edges) {
        successors.get(edge.from).push(edge.to);
    }

    const state = new Map();

    const visit = (id) => {
        if (state.get(id) === 'done') {
            return true;
        }

        if (state.get(id) === 'open') {
            return false;
        }

        state.set(id, 'open');

        for (const next of successors.get(id)) {
            if (!visit(next)) {
                return false;
            }
        }

        state.set(id, 'done');

        return true;
    };

    return nodes.every(visit);
}

const skeleton = `${repository}out/skeleton.json`;

test('a fresh Laravel skeleton breaks no rule and has no cycle', { skip: !existsSync(skeleton) && 'regenerate out/skeleton.json first' }, () => {
    const index = indexGraph(JSON.parse(readFileSync(skeleton, 'utf8')));

    assert.deepEqual(violations(index), []);
    assert.deepEqual(moduleCycles(index), []);
});

const framework = `${repository}out/framework.json`;

test('the rules describe applications: the framework itself is not checked against them', { skip: !existsSync(framework) && 'regenerate out/framework.json first' }, () => {
    const graph = JSON.parse(readFileSync(framework, 'utf8'));

    assert.equal(graph.verdict.rules_apply, false);
    assert.deepEqual(graph.verdict.violations, []);
    assert.equal(JSON.parse(readFileSync(skeleton, 'utf8')).verdict.rules_apply, true);
});

const graphs = { 'fixture/blog': blog, 'fixture/shop': shop };

for (const name of ['laravelio', 'framework']) {
    const path = `${repository}out/${name}.json`;

    if (existsSync(path)) {
        graphs[name] = indexGraph(JSON.parse(readFileSync(path, 'utf8')));
    }
}

for (const [name, index] of Object.entries(graphs)) {
    test(`${name}: every view counts exactly the violating class edges under each aggregated edge`, () => {
        const namespaces = new Set(index.graph.classes.filter((cls) => !cls.external).map((cls) => cls.namespace));
        const views = [{ level: 1 }, ...[...namespaces].map((focus) => ({ level: 2, focus }))];

        for (const view of views) {
            for (const edge of project(index, view).edges) {
                const underlying = edge.edges.filter((candidate) => index.violating.has(candidate)).length;

                assert.equal(edge.violations, underlying, `${JSON.stringify(view)} ${edge.id}`);
            }
        }
    });

    test(`${name}: the module view carries every violation`, () => {
        const total = project(index, { level: 1, budget: Infinity }).edges.reduce((sum, edge) => sum + edge.violations, 0);

        assert.equal(total, violations(index).length);
        assert.ok(DEFAULT_BUDGETS[1] > 0);
    });
}

test('the cheapest cut weighs the same as a brute-force minimum on the fixtures', () => {
    for (const index of [blog, shop]) {
        for (const cycle of moduleCycles(index)) {
            const weight = (edges) => edges.reduce((sum, edge) => sum + edge.weight, 0);
            let minimum = Infinity;

            for (let mask = 1; mask < (1 << cycle.edges.length); mask++) {
                const removed = cycle.edges.filter((_, position) => mask & (1 << position));
                const kept = cycle.edges.filter((_, position) => !(mask & (1 << position)));

                if (weight(removed) < minimum && acyclic({ nodes: cycle.modules, edges: kept })) {
                    minimum = weight(removed);
                }
            }

            assert.equal(cycle.exact, true);
            assert.equal(weight(cycle.cut), minimum, cycle.modules.join(', '));
        }
    }
});
