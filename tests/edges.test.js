import assert from 'node:assert/strict';
import { test } from 'node:test';

import { visibleEdges } from '../viewer/model.js';

const edge = (from, to, weight, cycle = false) => ({ id: `${from}->${to}`, from, to, weight, cycle });

const ids = (edges) => edges.map((edge) => edge.id).sort();

test('every edge is drawn when the budget allows it', () => {
    const edges = [edge('a', 'b', 1), edge('b', 'c', 2)];

    const { shown, hidden } = visibleEdges(edges, 10);

    assert.deepEqual(ids(shown), ['a->b', 'b->c']);
    assert.deepEqual(hidden, []);
});

test('the heaviest edges fill the budget and the rest are hidden, heaviest first', () => {
    const edges = [edge('a', 'b', 5), edge('a', 'c', 1), edge('b', 'c', 3), edge('c', 'a', 2)];

    const { shown, hidden } = visibleEdges(edges, 2);

    assert.deepEqual(ids(shown), ['a->b', 'b->c']);
    assert.deepEqual(hidden.map((edge) => edge.id), ['c->a', 'a->c']);
});

test('no node loses all of its edges, even past the budget', () => {
    const edges = [edge('a', 'b', 9), edge('a', 'c', 8), edge('d', 'e', 1)];

    const { shown } = visibleEdges(edges, 1);

    assert.deepEqual(ids(shown), ['a->b', 'a->c', 'd->e']);
});

test('cycle edges come before heavier edges but still respect the budget', () => {
    const edges = [edge('a', 'b', 9), edge('b', 'c', 8), edge('c', 'b', 1, true), edge('a', 'c', 1, true), edge('c', 'a', 1, true)];

    const { shown, hidden } = visibleEdges(edges, 3);

    assert.equal(shown.length, 3);
    assert.ok(shown.every((edge) => edge.cycle));
    assert.deepEqual(ids(hidden), ['a->b', 'b->c']);
});

test('shown and hidden partition the edges', () => {
    const edges = Array.from({ length: 50 }, (_, position) => edge(`n${position % 7}`, `n${(position * 3) % 11}`, position % 5, position % 4 === 0));

    const { shown, hidden } = visibleEdges(edges, 12);

    assert.equal(shown.length + hidden.length, edges.length);
    assert.equal(new Set([...shown, ...hidden]).size, edges.length);
});

test('edges that break a rule come before cycle edges and heavier edges', () => {
    const edges = [edge('a', 'b', 9, true), { ...edge('b', 'a', 1), violations: 1 }];

    const { shown, hidden } = visibleEdges(edges, 1);

    assert.deepEqual(ids(shown), ['b->a']);
    assert.deepEqual(ids(hidden), ['a->b']);
});

test('without cycles first, the heaviest edges win over cycle edges', () => {
    const edges = [edge('a', 'b', 9), edge('b', 'c', 8), edge('c', 'b', 1, true), edge('a', 'c', 1, true), edge('c', 'a', 1, true)];

    const { shown, hidden } = visibleEdges(edges, 3, { cyclesFirst: false });

    assert.deepEqual(ids(shown), ['a->b', 'a->c', 'b->c']);
    assert.deepEqual(ids(hidden), ['c->a', 'c->b']);
});
