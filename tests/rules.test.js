import assert from 'node:assert/strict';
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { DEFAULT_BUDGETS, indexGraph, moduleOf, project, WIRING_KINDS } from '../viewer/model.js';
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

test('blog golden: the same violations without vendor/, from composer.lock alone, as in CI', () => {
    const dir = mkdtempSync(join(tmpdir(), 'puml-lock-'));
    cpSync(`${repository}fixtures/blog`, dir, { recursive: true, filter: (path) => !/\/(vendor|expected)(\/|$)/.test(path) });
    const installed = readJson('fixtures/blog/vendor/composer/installed.json');
    writeFileSync(join(dir, 'composer.lock'), JSON.stringify({ packages: installed.packages, 'packages-dev': [] }));

    try {
        const locked = indexGraph(extract(dir));

        assert.ok(!existsSync(join(dir, 'vendor')));
        assert.deepEqual(violations(locked).flatMap(describe), expected.violations);
        assert.deepEqual(moduleCycles(locked).map((cycle) => cycle.modules), expected.cycles);
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
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

for (const [name, index] of Object.entries(graphs)) {
    const { graph } = index;
    const home = new Map();

    for (const cycle of moduleCycles(index)) {
        for (const fold of cycle.folds) {
            home.set(fold.class, fold.into);
        }
    }

    const moduleWithFolds = (id) => home.get(id) ?? moduleOf(index.classes.get(id)).id;

    test(`${name}: every fold is a trait, interface or abstract class whose users all belong to the module it is counted in`, () => {
        for (const cycle of moduleCycles(index)) {
            for (const fold of cycle.folds) {
                const cls = index.classes.get(fold.class);
                const users = graph.edges.filter((edge) => edge.to === fold.class && ['extends', 'implements', 'uses-trait'].includes(edge.kind));

                assert.ok(cls.kind === 'trait' || cls.kind === 'interface' || cls.abstract === true, fold.class);
                assert.equal(moduleOf(cls).id, fold.module, fold.class);
                assert.notEqual(fold.into, fold.module, fold.class);
                assert.equal(users.length, fold.users, fold.class);
                assert.ok(users.length > 0, fold.class);
                assert.ok(users.every((edge) => moduleWithFolds(edge.from) === fold.into), fold.class);
            }
        }
    });

    test(`${name}: the cut edges carry exactly the class edges behind them, and the cost sums them`, () => {
        for (const cycle of moduleCycles(index)) {
            const modules = new Set(cycle.modules);
            let classEdges = 0;
            let violations = 0;
            const files = new Set();

            for (const edge of cycle.cut) {
                const behind = graph.edges
                    .map((candidate, position) => ({ candidate, position }))
                    .filter(({ candidate }) => moduleWithFolds(candidate.from) === edge.from && moduleWithFolds(candidate.to) === edge.to)
                    .filter(({ candidate }) => !WIRING_KINDS.has(candidate.kind))
                    .filter(({ candidate }) => !index.classes.get(candidate.from).external || index.classes.get(candidate.from).root != null)
                    .filter(({ candidate }) => !index.classes.get(candidate.to).external || index.classes.get(candidate.to).root != null)
                    .map(({ position }) => position);

                assert.ok(modules.has(edge.from) && modules.has(edge.to), `${edge.from} -> ${edge.to}`);
                assert.deepEqual([...edge.edges].sort((a, b) => a - b), behind.sort((a, b) => a - b), `${edge.from} -> ${edge.to}`);
                assert.equal(edge.weight, edge.edges.length);

                classEdges += edge.weight;
                violations += edge.violations;

                for (const position of edge.edges) {
                    for (const place of graph.edges[position].evidence) {
                        files.add(place.file);
                    }
                }
            }

            assert.deepEqual(cycle.cost, { class_edges: classEdges, files: files.size, violations });
            assert.ok(cycle.remaining.length > 1);
            assert.ok(cycle.remaining.every((id) => modules.has(id)));
            assert.ok(cycle.edges.every((edge) => cycle.remaining.includes(edge.from) && cycle.remaining.includes(edge.to)));
        }
    });
}
