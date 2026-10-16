/**
 * Projects a puml graph (the versioned JSON written by puml-extract) onto one level of detail.
 *
 * Level 1, modules: a node is a module, i.e. a PSR-4 root plus the next namespace segment
 * (App\Models, Illuminate\Database), or one whole vendor package (laravel/framework), or "php" for
 * built-in classes. Every module that declares a class is shown; packages only when an edge reaches them.
 *
 * Level 2, one step down the namespace tree: the focus is a namespace (or package). Each class declared
 * directly in it is a node, and each child namespace is one node that can be drilled into in turn. A child
 * namespace that only holds a single namespace of its own is skipped, so App\Domain\Billing is one step
 * when App\Domain holds nothing else. Everything outside the focus that shares an edge with it is shown at
 * the granularity of a sibling: a class of another module as its module, and a class of the same module as
 * the child of the deepest namespace it shares with the focus (or as itself when it sits directly in it).
 *
 * Level 3, UML: the focus is a class (or every class of a namespace). Focused classes are drawn with their
 * members; each class sharing an edge with them is shown as a plain class node.
 *
 * Any projected edge can be drilled into: the edge view shows the classes on both of its ends and the class
 * edges between them, and nothing else.
 *
 * At every level an edge between two nodes aggregates every class edge between their classes: its weight
 * is the number of class edges, and it keeps them as evidence. Levels 2 and 3 only show edges touching the
 * focus. Cycles are the strongly connected components (Tarjan) of the projected graph. When there are
 * more nodes than the budget, the lowest-ranked nodes merge into a single "other" node; cycles are found
 * before that merge so it cannot invent any. The budget is 40 nodes at levels 1 and 2 and 24 at level 3,
 * where the focused class alone takes the room of many plain nodes.
 *
 * Drawing every aggregated edge is not legible either, so the renderer draws an edge budget of them: first
 * the heaviest edge of each node, so no node loses all of its edges, then cycle edges before the others,
 * each by weight, until the budget is reached. Hidden edges stay in the projection and are listed, never
 * dropped.
 */

export const DEFAULT_BUDGETS = { 1: 40, 2: 40, 3: 24 };

export const DEFAULT_EDGE_BUDGET = 60;

export const EDGE_KINDS = [
    'extends',
    'implements',
    'uses-trait',
    'injects',
    'property',
    'signature',
    'relation',
    'new',
    'static',
    'facade',
    'reference',
];

export function indexGraph(graph) {
    const classes = new Map(graph.classes.map((cls) => [cls.id, cls]));
    const modules = new Map(graph.classes.map((cls) => [cls.id, moduleOf(cls)]));
    const outgoing = new Map();
    const incoming = new Map();

    for (const edge of graph.edges) {
        push(outgoing, edge.from, edge);
        push(incoming, edge.to, edge);
    }

    return { graph, classes, modules, moduleIds: new Set([...modules.values()].map((module) => module.id)), namespaces: namespaceTree(graph), outgoing, incoming };
}

function namespaceTree(graph) {
    const namespaces = new Map();

    const entry = (namespace) => {
        if (!namespaces.has(namespace)) {
            namespaces.set(namespace, { classes: [], children: new Set() });
        }

        return namespaces.get(namespace);
    };

    for (const cls of graph.classes.filter((cls) => !cls.external)) {
        entry(cls.namespace).classes.push(cls.id);

        let child = cls.namespace;

        while (child.includes('\\')) {
            const parent = child.slice(0, child.lastIndexOf('\\'));

            entry(parent).children.add(child);
            child = parent;
        }
    }

    return namespaces;
}

export function childNamespace(index, parent, namespace) {
    const segments = namespace.slice(parent.length + 1).split('\\');
    let child = `${parent}\\${segments[0]}`;

    for (const segment of segments.slice(1)) {
        if (!passesThrough(index, child)) {
            break;
        }

        child = `${child}\\${segment}`;
    }

    return child;
}

function passesThrough(index, namespace) {
    const entry = index.namespaces.get(namespace);

    return entry !== undefined && entry.classes.length === 0 && entry.children.size === 1;
}

function commonNamespace(a, b) {
    const left = a.split('\\');
    const right = b.split('\\');
    let length = 0;

    while (length < left.length && length < right.length && left[length] === right[length]) {
        length++;
    }

    return left.slice(0, length).join('\\');
}

function moduleContaining(index, namespace) {
    const modules = [...index.moduleIds].filter((id) => namespace === id || namespace.startsWith(`${id}\\`));

    return modules.sort((a, b) => b.length - a.length)[0] ?? null;
}

export function parentView(index, view) {
    if (view.edge) {
        return { ...view, edge: null };
    }

    const level = view.level ?? 1;

    if (level === 1) {
        return null;
    }

    const focus = view.focus ?? defaultFocus(index, level);
    const cls = index.classes.get(focus);

    if (cls) {
        return { ...view, level: 2, focus: cls.external && cls.root == null ? cls.package : cls.namespace };
    }

    if (level === 3) {
        return { ...view, level: 2, focus };
    }

    if (index.moduleIds.has(focus) || !focus.includes('\\')) {
        return { ...view, level: 1, focus: null };
    }

    let parent = focus.slice(0, focus.lastIndexOf('\\'));

    while (!index.moduleIds.has(parent) && parent.includes('\\') && passesThrough(index, parent)) {
        parent = parent.slice(0, parent.lastIndexOf('\\'));
    }

    return { ...view, level: 2, focus: parent };
}

export function moduleOf(cls) {
    if (cls.external && cls.root == null) {
        return { id: cls.package, kind: 'package' };
    }

    const namespace = cls.namespace;
    const root = cls.root ?? '';

    if (namespace === root) {
        return { id: root || '(global)', kind: 'module' };
    }

    const rest = root === '' ? namespace : namespace.slice(root.length + 1);
    const segment = rest.split('\\')[0];

    return { id: root === '' ? segment : `${root}\\${segment}`, kind: 'module' };
}

export function defaultFocus(index, level) {
    const declared = index.graph.classes.filter((cls) => !cls.external);

    const counts = new Map();

    for (const cls of declared) {
        const module = index.modules.get(cls.id).id;
        counts.set(module, (counts.get(module) ?? 0) + 1);
    }

    const largest = [...counts].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0]?.[0] ?? null;

    if (level < 3 || largest === null) {
        return largest;
    }

    const degree = (cls) => (index.outgoing.get(cls.id)?.length ?? 0) + (index.incoming.get(cls.id)?.length ?? 0);

    return declared
        .filter((cls) => index.modules.get(cls.id).id === largest)
        .sort((a, b) => degree(b) - degree(a) || a.id.localeCompare(b.id))[0].id;
}

export function project(index, view) {
    if (view.edge) {
        return projectEdge(index, view);
    }

    const level = view.level ?? 1;
    const budget = view.budget ?? DEFAULT_BUDGETS[level];
    const vendor = view.vendor ?? true;
    const kinds = new Set(view.kinds ?? EDGE_KINDS);
    const focus = level === 1 ? null : (view.focus ?? defaultFocus(index, level));

    const nodeOf = nodeMapper(index, level, focus, vendor);
    const nodes = new Map();
    const edges = new Map();

    const addNode = (node) => {
        if (!nodes.has(node.id)) {
            nodes.set(node.id, { ...node, classes: [] });
        }

        return nodes.get(node.id);
    };

    for (const cls of index.graph.classes) {
        const node = nodeOf(cls);

        if (node && !cls.external && (level === 1 || node.focus)) {
            addNode(node).classes.push(cls.id);
        }
    }

    for (const edge of index.graph.edges) {
        if (!kinds.has(edge.kind)) {
            continue;
        }

        const from = nodeOf(index.classes.get(edge.from));
        const to = nodeOf(index.classes.get(edge.to));

        if (!from || !to || from.id === to.id || (level > 1 && !from.focus && !to.focus)) {
            continue;
        }

        for (const [node, id] of [[from, edge.from], [to, edge.to]]) {
            const visible = addNode(node);

            if (!visible.classes.includes(id)) {
                visible.classes.push(id);
            }
        }

        aggregate(edges, from.id, to.id, edge);
    }

    markCycles([...nodes.values()], [...edges.values()]);

    const protect = level === 1 ? (node) => node.kind === 'module' : (node) => node.focus === true;
    const projection = enforceBudget([...nodes.values()], [...edges.values()], budget, protect);

    return { level, focus, budget, edge: null, ...projection };
}

function projectEdge(index, view) {
    const base = project(index, { ...view, edge: null });
    const drilled = base.edges.find((edge) => edge.from === view.edge.from && edge.to === view.edge.to);
    const nodes = new Map();
    const edges = new Map();

    for (const edge of drilled?.edges ?? []) {
        for (const [id, side] of [[edge.from, 'from'], [edge.to, 'to']]) {
            if (!nodes.has(id)) {
                nodes.set(id, { id, kind: 'class', label: index.classes.get(id).name, focus: false, side, classes: [id] });
            }
        }

        aggregate(edges, edge.from, edge.to, edge);
    }

    markCycles([...nodes.values()], [...edges.values()]);

    const projection = enforceBudget([...nodes.values()], [...edges.values()], base.budget, () => false);

    return { level: base.level, focus: base.focus, budget: base.budget, edge: { from: view.edge.from, to: view.edge.to, weight: drilled?.weight ?? 0 }, ...projection };
}

function aggregate(edges, from, to, edge) {
    const key = `${from}\u0000${to}`;

    if (!edges.has(key)) {
        edges.set(key, { id: key, from, to, weight: 0, kinds: {}, edges: [], cycle: false });
    }

    const projected = edges.get(key);
    projected.weight++;
    projected.kinds[edge.kind] = (projected.kinds[edge.kind] ?? 0) + 1;
    projected.edges.push(edge);
}

function nodeMapper(index, level, focus, vendor) {
    const cache = new Map();

    const inNamespace = (cls, namespace) =>
        !(cls.external && cls.root == null) && (cls.namespace === namespace || cls.namespace.startsWith(`${namespace}\\`));

    const inFocus = (cls) => {
        if (level === 3 && index.classes.has(focus) && !index.classes.get(focus).external) {
            return cls.id === focus;
        }

        return cls.external && cls.root == null ? cls.package === focus : inNamespace(cls, focus);
    };

    const focusModule = level === 2 ? moduleContaining(index, focus) : null;

    const map = (cls) => {
        const module = index.modules.get(cls.id);
        const isPackage = module.kind === 'package';

        if (level > 1 && inFocus(cls)) {
            if (level === 3) {
                return { id: cls.id, kind: 'uml', label: cls.name, focus: true };
            }

            if (isPackage || cls.namespace === focus) {
                return { id: cls.id, kind: 'class', label: cls.name, focus: true };
            }

            const namespace = childNamespace(index, focus, cls.namespace);

            return { id: namespace, kind: 'namespace', label: namespace, focus: true };
        }

        if (isPackage && !vendor) {
            return null;
        }

        if (level === 3) {
            return { id: cls.id, kind: 'class', label: cls.name, focus: false };
        }

        if (level === 1 || module.id !== focusModule) {
            return { id: module.id, kind: module.kind, label: module.id, focus: false };
        }

        const shared = commonNamespace(cls.namespace, focus);

        if (cls.namespace === shared) {
            return { id: cls.id, kind: 'class', label: cls.name, focus: false };
        }

        const sibling = childNamespace(index, shared, cls.namespace);

        return { id: sibling, kind: 'namespace', label: sibling, focus: false };
    };

    return (cls) => {
        if (!cls) {
            return null;
        }

        if (!cache.has(cls.id)) {
            cache.set(cls.id, map(cls));
        }

        return cache.get(cls.id);
    };
}

export function stronglyConnected(nodeIds, edges) {
    const successors = new Map(nodeIds.map((id) => [id, []]));

    for (const edge of edges) {
        successors.get(edge.from)?.push(edge.to);
    }

    const order = new Map();
    const lowlink = new Map();
    const stack = [];
    const onStack = new Set();
    const components = [];
    let counter = 0;

    const visit = (id) => {
        order.set(id, counter);
        lowlink.set(id, counter);
        counter++;
        stack.push(id);
        onStack.add(id);

        for (const next of successors.get(id) ?? []) {
            if (!order.has(next)) {
                visit(next);
                lowlink.set(id, Math.min(lowlink.get(id), lowlink.get(next)));
            } else if (onStack.has(next)) {
                lowlink.set(id, Math.min(lowlink.get(id), order.get(next)));
            }
        }

        if (lowlink.get(id) === order.get(id)) {
            const component = [];
            let member;

            do {
                member = stack.pop();
                onStack.delete(member);
                component.push(member);
            } while (member !== id);

            components.push(component);
        }
    };

    for (const id of nodeIds) {
        if (!order.has(id)) {
            visit(id);
        }
    }

    return components;
}

function markCycles(nodes, edges) {
    const componentOf = new Map();

    for (const component of stronglyConnected(nodes.map((node) => node.id), edges)) {
        if (component.length > 1) {
            for (const id of component) {
                componentOf.set(id, component);
            }
        }
    }

    for (const node of nodes) {
        node.cycle = componentOf.has(node.id);
    }

    for (const edge of edges) {
        edge.cycle = componentOf.has(edge.from) && componentOf.get(edge.from) === componentOf.get(edge.to);
    }
}

function enforceBudget(nodes, edges, budget, protect) {
    if (nodes.length <= budget) {
        return { nodes: sortById(nodes), edges: sortById(edges), merged: [] };
    }

    const weight = new Map(nodes.map((node) => [node.id, 0]));

    for (const edge of edges) {
        weight.set(edge.from, weight.get(edge.from) + edge.weight);
        weight.set(edge.to, weight.get(edge.to) + edge.weight);
    }

    const ranked = [...nodes].sort(
        (a, b) =>
            Number(protect(b)) - Number(protect(a)) ||
            weight.get(b.id) - weight.get(a.id) ||
            a.id.localeCompare(b.id),
    );

    const kept = ranked.slice(0, budget - 1);
    const merged = ranked.slice(budget - 1);
    const mergedIds = new Set(merged.map((node) => node.id));

    const other = {
        id: 'other',
        kind: 'other',
        label: `${merged.length} more`,
        focus: false,
        cycle: false,
        classes: merged.flatMap((node) => node.classes),
        members: merged.map((node) => node.id),
    };

    const rerouted = new Map();

    for (const edge of edges) {
        const from = mergedIds.has(edge.from) ? 'other' : edge.from;
        const to = mergedIds.has(edge.to) ? 'other' : edge.to;

        if (from === to) {
            continue;
        }

        const key = `${from}\u0000${to}`;

        if (!rerouted.has(key)) {
            rerouted.set(key, { id: key, from, to, weight: 0, kinds: {}, edges: [], cycle: false });
        }

        const target = rerouted.get(key);
        target.weight += edge.weight;
        target.edges.push(...edge.edges);
        target.cycle ||= edge.cycle;

        for (const [kind, count] of Object.entries(edge.kinds)) {
            target.kinds[kind] = (target.kinds[kind] ?? 0) + count;
        }
    }

    return { nodes: sortById([...kept, other]), edges: sortById([...rerouted.values()]), merged: merged.map((node) => node.id) };
}

export function visibleEdges(edges, budget = DEFAULT_EDGE_BUDGET) {
    const heaviestFirst = [...edges].sort(
        (a, b) => Number(Boolean(b.cycle)) - Number(Boolean(a.cycle)) || b.weight - a.weight || a.id.localeCompare(b.id),
    );
    const shown = new Set();
    const covered = new Set();

    for (const edge of heaviestFirst) {
        if (!covered.has(edge.from) || !covered.has(edge.to)) {
            shown.add(edge);
            covered.add(edge.from).add(edge.to);
        }
    }

    for (const edge of heaviestFirst) {
        if (shown.size >= budget) {
            break;
        }

        shown.add(edge);
    }

    return {
        shown: edges.filter((edge) => shown.has(edge)),
        hidden: heaviestFirst.filter((edge) => !shown.has(edge)),
    };
}

function sortById(items) {
    return items.sort((a, b) => a.id.localeCompare(b.id));
}

function push(map, key, value) {
    if (!map.has(key)) {
        map.set(key, []);
    }

    map.get(key).push(value);
}

const VISIBILITY = { public: '+', protected: '#', private: '-' };

export function formatProperty(property) {
    const type = property.type ? `: ${property.type}` : '';
    const modifiers = [property.static && 'static', property.readonly && 'readonly'].filter(Boolean).join(' ');

    return `${VISIBILITY[property.visibility] ?? ''}${property.name}${type}${modifiers ? ` ${modifiers}` : ''}`;
}

export function formatMethod(method) {
    const params = method.params.map((param) => (param.type ? `${param.name}: ${param.type}` : param.name)).join(', ');
    const returns = method.returns ? `: ${method.returns}` : '';

    return `${VISIBILITY[method.visibility] ?? ''}${method.name}(${params})${returns}`;
}

export function shortTypes(text) {
    return text.replace(/\\?(?:[A-Za-z_][A-Za-z0-9_]*\\)+([A-Za-z_][A-Za-z0-9_]*)/g, '$1');
}
