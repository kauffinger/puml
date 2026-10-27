import assert from 'node:assert/strict';
import { test } from 'node:test';

import { stronglyConnected } from '../viewer/model.js';

const components = (nodes, pairs) =>
    stronglyConnected(nodes, pairs.map(([from, to]) => ({ from, to })))
        .map((component) => component.sort().join(''))
        .sort();

test('an acyclic graph has only single-node components', () => {
    assert.deepEqual(components(['a', 'b', 'c'], [['a', 'b'], ['b', 'c'], ['a', 'c']]), ['a', 'b', 'c']);
});

test('a two-node cycle is one component', () => {
    assert.deepEqual(components(['a', 'b', 'c'], [['a', 'b'], ['b', 'a'], ['b', 'c']]), ['ab', 'c']);
});

test('separate cycles joined by a one-way edge stay separate', () => {
    const edges = [['a', 'b'], ['b', 'c'], ['c', 'a'], ['c', 'd'], ['d', 'e'], ['e', 'd']];

    assert.deepEqual(components(['a', 'b', 'c', 'd', 'e'], edges), ['abc', 'de']);
});

test('a self-contained chain closing back on its start is one component', () => {
    assert.deepEqual(components(['a', 'b', 'c', 'd'], [['a', 'b'], ['b', 'c'], ['c', 'd'], ['d', 'a']]), ['abcd']);
});

test('a wiring edge does not close a cycle: an event that names its listener does not depend on it', async () => {
    const { indexGraph, project } = await import('../viewer/model.js');
    const declare = (id) => ({ id, name: id.split('\\').pop(), namespace: id.slice(0, id.lastIndexOf('\\')), root: 'App', external: false, package: 'app', kind: 'class' });
    const relate = (from, to, kind) => ({ from, to, kind, evidence: [{ file: 'app.php', line: 1 }] });

    const index = indexGraph({
        roots: ['App'],
        packages: [{ name: 'app', kind: 'app' }],
        verdict: { violations: [] },
        classes: ['App\\Events\\Published', 'App\\Listeners\\Notify'].map(declare),
        edges: [relate('App\\Listeners\\Notify', 'App\\Events\\Published', 'signature'), relate('App\\Events\\Published', 'App\\Listeners\\Notify', 'listener')],
    });
    const view = project(index, { level: 1 });

    assert.equal(view.edges.length, 2);
    assert.ok(view.edges.every((edge) => !edge.cycle));
    assert.ok(view.nodes.every((node) => !node.cycle));
});
