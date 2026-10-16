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
