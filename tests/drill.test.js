import assert from 'node:assert/strict';
import { test } from 'node:test';

import { indexGraph, parentView, project } from '../viewer/model.js';
import { describeEdges, describeNodes } from './support.js';

const declare = (id) => {
    const namespace = id.slice(0, id.lastIndexOf('\\'));

    return { id, name: id.slice(id.lastIndexOf('\\') + 1), namespace, root: 'App', external: false, package: 'app', kind: 'class' };
};

const relate = (from, to, kind) => ({ from, to, kind, evidence: [{ file: 'app.php', line: 1 }] });

const index = indexGraph({
    roots: ['App'],
    classes: [
        'App\\Http\\Kernel',
        'App\\Http\\Controllers\\UserController',
        'App\\Http\\Controllers\\Admin\\Blog\\PostController',
        'App\\Http\\Requests\\StoreUser',
        'App\\Http\\Middleware\\Authenticate',
        'App\\Models\\User',
        'App\\Models\\Post',
    ].map(declare),
    edges: [
        relate('App\\Http\\Kernel', 'App\\Http\\Middleware\\Authenticate', 'reference'),
        relate('App\\Http\\Kernel', 'App\\Http\\Controllers\\UserController', 'reference'),
        relate('App\\Http\\Controllers\\UserController', 'App\\Http\\Requests\\StoreUser', 'injects'),
        relate('App\\Http\\Controllers\\UserController', 'App\\Models\\User', 'signature'),
        relate('App\\Http\\Controllers\\Admin\\Blog\\PostController', 'App\\Models\\Post', 'new'),
        relate('App\\Http\\Controllers\\Admin\\Blog\\PostController', 'App\\Models\\User', 'signature'),
    ],
});

test('a namespace view shows its own classes and one node per child namespace', () => {
    const projection = project(index, { level: 2, focus: 'App\\Http' });

    assert.deepEqual(describeNodes(projection), [
        'class App\\Http\\Kernel',
        'module App\\Models',
        'namespace App\\Http\\Controllers',
        'namespace App\\Http\\Middleware',
        'namespace App\\Http\\Requests',
    ]);

    assert.deepEqual(describeEdges(projection), [
        'App\\Http\\Controllers -> App\\Http\\Requests 1',
        'App\\Http\\Controllers -> App\\Models 3',
        'App\\Http\\Kernel -> App\\Http\\Controllers 1',
        'App\\Http\\Kernel -> App\\Http\\Middleware 1',
    ]);
});

test('a child namespace holding only one namespace is skipped', () => {
    const projection = project(index, { level: 2, focus: 'App\\Http\\Controllers' });

    assert.ok(describeNodes(projection).includes('namespace App\\Http\\Controllers\\Admin\\Blog'));
    assert.ok(!describeNodes(projection).some((node) => node.endsWith('Controllers\\Admin')));
});

test('neighbours in the same module are shown as siblings, neighbours elsewhere as their module', () => {
    const projection = project(index, { level: 2, focus: 'App\\Http\\Controllers' });

    assert.deepEqual(describeNodes(projection), [
        'class App\\Http\\Controllers\\UserController',
        'class App\\Http\\Kernel',
        'module App\\Models',
        'namespace App\\Http\\Controllers\\Admin\\Blog',
        'namespace App\\Http\\Requests',
    ]);

    assert.deepEqual(describeEdges(projection), [
        'App\\Http\\Controllers\\Admin\\Blog -> App\\Models 2',
        'App\\Http\\Controllers\\UserController -> App\\Http\\Requests 1',
        'App\\Http\\Controllers\\UserController -> App\\Models 1',
        'App\\Http\\Kernel -> App\\Http\\Controllers\\UserController 1',
    ]);
});

test('going up walks from a class to its namespace, through skipped namespaces, to the modules', () => {
    const up = (view) => parentView(index, view);

    assert.deepEqual(up({ level: 3, focus: 'App\\Http\\Controllers\\UserController' }), { level: 2, focus: 'App\\Http\\Controllers' });
    assert.deepEqual(up({ level: 2, focus: 'App\\Http\\Controllers\\Admin\\Blog' }), { level: 2, focus: 'App\\Http\\Controllers' });
    assert.deepEqual(up({ level: 2, focus: 'App\\Http\\Controllers' }), { level: 2, focus: 'App\\Http' });
    assert.deepEqual(up({ level: 2, focus: 'App\\Http' }), { level: 1, focus: null });
    assert.equal(up({ level: 1 }), null);
});

test('an edge view shows only the classes and class edges behind one edge', () => {
    const view = { level: 2, focus: 'App\\Http', edge: { from: 'App\\Http\\Controllers', to: 'App\\Models' } };
    const projection = project(index, view);

    assert.deepEqual(projection.nodes.map((node) => `${node.side} ${node.id}`).sort(), [
        'from App\\Http\\Controllers\\Admin\\Blog\\PostController',
        'from App\\Http\\Controllers\\UserController',
        'to App\\Models\\Post',
        'to App\\Models\\User',
    ]);

    assert.deepEqual(describeEdges(projection), [
        'App\\Http\\Controllers\\Admin\\Blog\\PostController -> App\\Models\\Post 1',
        'App\\Http\\Controllers\\Admin\\Blog\\PostController -> App\\Models\\User 1',
        'App\\Http\\Controllers\\UserController -> App\\Models\\User 1',
    ]);

    assert.deepEqual(projection.edge, { from: 'App\\Http\\Controllers', to: 'App\\Models', weight: 3 });
    assert.deepEqual(parentView(index, view), { level: 2, focus: 'App\\Http', edge: null });
});
