import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { test } from 'node:test';

import { WIRING_KINDS } from '../viewer/model.js';
import { extract, readJson, repository } from './support.js';

const fixture = extract(`${repository}fixtures/wiring`);
const expected = readJson('fixtures/wiring/expected/laravel.json');

test('the fixture wires up what Laravel registers, each fact on the line that registers it', () => {
    for (const section of ['routes', 'listeners', 'observers', 'policies', 'bindings', 'schedule', 'middleware']) {
        const sorted = (facts) => facts.map((fact) => JSON.stringify(fact)).sort();

        assert.deepEqual(sorted(fixture.laravel[section]), sorted(expected[section]), section);
    }
});

test('a route registered under a condition is noted on its own line, not read', () => {
    assert.deepEqual(
        fixture.unresolved.map(({ kind, file, line }) => ({ kind, file, line })),
        expected.unresolved,
    );
    assert.ok(!fixture.laravel.routes.some((route) => route.uri === 'debug'));
});

test('route files and bootstrap/app.php are entry nodes', () => {
    assert.deepEqual(
        fixture.classes.filter((cls) => cls.kind === 'file').map((cls) => [cls.id, cls.layer]),
        [
            ['bootstrap/app.php', 'entry'],
            ['routes/api.php', 'entry'],
            ['routes/console.php', 'entry'],
            ['routes/web.php', 'entry'],
        ],
    );
});

test('a route reaches the middleware its alias names', () => {
    const edge = fixture.edges.find((edge) => edge.kind === 'middleware' && edge.from === 'routes/web.php');

    assert.equal(edge.to, 'App\\Http\\Middleware\\EnsureAdmin');
    assert.equal(edge.label, 'route:admin');
});

const graphs = { 'fixture/wiring': fixture };

for (const name of ['laravelio', 'framework']) {
    const path = `${repository}out/${name}.json`;

    if (existsSync(path)) {
        graphs[name] = JSON.parse(readFileSync(path, 'utf8'));
    }
}

/** The edges each fact of the laravel section stands for, as from, to and kind. */
function factEdges(graph) {
    const { routes, listeners, observers, policies, bindings, schedule, middleware } = graph.laravel;
    const aliases = new Map(middleware.filter((fact) => fact.slot.startsWith('alias:')).map((fact) => [fact.slot.slice(6), fact.class]));
    const edges = [];

    for (const route of routes) {
        if (route.target) {
            edges.push([route.from, route.target, 'route', route.file, route.line]);
        }

        for (const name of route.middleware.map((entry) => entry.split(':')[0])) {
            const target = name.includes('\\') ? name : aliases.get(name);

            if (target) {
                edges.push([route.from, target, 'middleware', route.file, route.line]);
            }
        }
    }

    for (const listener of listeners.filter((listener) => listener.class)) {
        edges.push([listener.event, listener.class, 'listener', listener.file, listener.line]);
    }

    for (const observer of observers) {
        edges.push([observer.model, observer.observer, 'observer', observer.file, observer.line]);
    }

    for (const policy of policies) {
        edges.push([policy.model, policy.policy, 'policy', policy.file, policy.line]);
    }

    for (const binding of bindings.filter((binding) => binding.concrete && binding.concrete !== binding.key)) {
        edges.push([binding.key, binding.concrete, 'binding', binding.file, binding.line]);
    }

    for (const task of schedule.filter((task) => task.class)) {
        edges.push([task.from, task.class, 'schedule', task.file, task.line]);
    }

    for (const fact of middleware) {
        edges.push([fact.from, fact.class, 'middleware', fact.file, fact.line]);
    }

    return edges;
}

for (const [name, graph] of Object.entries(graphs)) {
    test(`${name}: every wiring edge stands for a fact of the laravel section, and every fact has its edge`, () => {
        const ids = new Map(graph.classes.map((cls) => [cls.id.toLowerCase(), cls.id]));
        const canonical = (id) => ids.get(id.toLowerCase()) ?? id;
        const key = ([from, to, kind, file, line]) => `${canonical(from)} -> ${canonical(to)} ${kind} ${file}:${line}`;

        const fromFacts = new Set(factEdges(graph).map(key));
        const fromEdges = new Set(
            graph.edges
                .filter((edge) => WIRING_KINDS.has(edge.kind))
                .flatMap((edge) => edge.evidence.map((place) => key([edge.from, edge.to, edge.kind, place.file, place.line]))),
        );

        assert.deepEqual([...fromEdges].filter((edge) => !fromFacts.has(edge)), [], 'wiring edges without a fact');
        assert.deepEqual([...fromFacts].filter((edge) => !fromEdges.has(edge)), [], 'facts without a wiring edge');
    });

    test(`${name}: wiring is never a dependency, so no rule violation is a wiring edge`, () => {
        for (const violation of graph.verdict.violations) {
            assert.ok(!WIRING_KINDS.has(graph.edges[violation.edge].kind), JSON.stringify(violation));
        }
    });

    test(`${name}: every edge leaves a node of the graph and enters one`, () => {
        const ids = new Set(graph.classes.map((cls) => cls.id));

        for (const edge of graph.edges) {
            assert.ok(ids.has(edge.from) && ids.has(edge.to), `${edge.from} -> ${edge.to}`);
        }
    });
}
