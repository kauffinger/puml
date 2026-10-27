import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { test } from 'node:test';

import { DEFAULT_BUDGETS, indexGraph, moduleOf, parentView, project } from '../viewer/model.js';
import { extract, repository } from './support.js';

const graphs = { 'fixture/shop': extract(`${repository}fixtures/shop`), 'fixture/wiring': extract(`${repository}fixtures/wiring`) };

for (const name of ['laravelio', 'framework']) {
    const path = `${repository}out/${name}.json`;

    if (existsSync(path)) {
        graphs[name] = JSON.parse(readFileSync(path, 'utf8'));
    }
}

function views(graph) {
    const declared = graph.classes.filter((cls) => !cls.external);
    const namespaces = new Set();

    for (const cls of declared) {
        const parts = cls.namespace.split('\\');

        for (let length = 1; length <= parts.length; length++) {
            namespaces.add(parts.slice(0, length).join('\\'));
        }
    }

    const modules = project(indexGraph(graph), { level: 1 });

    return [
        { level: 1 },
        { level: 1, vendor: false },
        ...[...namespaces].map((focus) => ({ level: 2, focus })),
        ...declared.map((cls) => ({ level: 3, focus: cls.id })),
        ...modules.edges.map((edge) => ({ level: 1, edge: { from: edge.from, to: edge.to } })),
    ];
}

for (const [name, graph] of Object.entries(graphs)) {
    const index = indexGraph(graph);
    const projections = views(graph).map((view) => ({ view, projection: project(index, view) }));

    test(`${name}: every aggregated edge weight is the number of class edges under it`, () => {
        for (const { view, projection } of projections) {
            for (const edge of projection.edges) {
                const byKind = Object.values(edge.kinds).reduce((sum, count) => sum + count, 0);

                assert.equal(edge.weight, edge.edges.length, `${JSON.stringify(view)} ${edge.id}`);
                assert.equal(edge.weight, byKind, `${JSON.stringify(view)} ${edge.id}`);
            }
        }
    });

    test(`${name}: every class edge is counted exactly once and belongs to its endpoints`, () => {
        for (const { view, projection } of projections) {
            const nodes = new Map(projection.nodes.map((node) => [node.id, node]));
            const seen = new Set();

            for (const edge of projection.edges) {
                for (const underlying of edge.edges) {
                    assert.ok(!seen.has(underlying), `${JSON.stringify(view)} counts an edge twice`);
                    seen.add(underlying);

                    assert.ok(nodes.get(edge.from).classes.includes(underlying.from), `${JSON.stringify(view)} ${underlying.from} not in ${edge.from}`);
                    assert.ok(nodes.get(edge.to).classes.includes(underlying.to), `${JSON.stringify(view)} ${underlying.to} not in ${edge.to}`);
                }
            }
        }
    });

    test(`${name}: module edges sum to every class edge that crosses a module`, () => {
        const crossing = graph.edges.filter((edge) => {
            const from = moduleOf(index.classes.get(edge.from)).id;
            const to = moduleOf(index.classes.get(edge.to)).id;

            return from !== to;
        });

        const projection = project(index, { level: 1, budget: Infinity });
        const total = projection.edges.reduce((sum, edge) => sum + edge.weight, 0);

        assert.equal(total, crossing.length);
    });

    test(`${name}: an edge view holds exactly the class edges of the edge it drills into`, () => {
        for (const edge of project(index, { level: 1, budget: Infinity }).edges) {
            const drilled = project(index, { level: 1, budget: Infinity, edge: { from: edge.from, to: edge.to } });
            const underlying = drilled.edges.flatMap((projected) => projected.edges);

            assert.equal(underlying.length, edge.weight, edge.id);
            assert.deepEqual(new Set(underlying), new Set(edge.edges), edge.id);
        }
    });

    test(`${name}: going up always ends at the modules`, () => {
        for (const { view } of projections.filter(({ view }) => !view.edge)) {
            let current = view;

            for (let steps = 0; current.level !== 1; steps++) {
                assert.ok(steps < 20, `${JSON.stringify(view)} does not reach the modules`);
                current = parentView(index, current);
            }
        }
    });

    test(`${name}: every view stays within the node budget`, () => {
        for (const { view, projection } of projections) {
            const budget = DEFAULT_BUDGETS[projection.level];

            assert.ok(projection.nodes.length <= budget, `${JSON.stringify(view)} has ${projection.nodes.length} of ${budget} nodes`);
        }
    });
}
