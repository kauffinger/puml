import { DEFAULT_EDGE_BUDGET, EDGE_KINDS, indexGraph, parentView, project, visibleEdges, formatProperty, formatMethod, shortTypes } from './model.js';

const MEMBER_LIMIT = 24;
const CONTENTS_LIMIT = 4;
const TOOLTIP_PAIRS = 12;
const LINE_LIMIT = 72;
const EVIDENCE_LIMIT = 400;
const LEGIBLE_SCALE = 0.8;

const LEVEL_NAMES = { 1: 'Modules', 2: 'Classes', 3: 'UML' };

const canvas = document.getElementById('canvas');
const panel = document.getElementById('panel');

let index;
let viz;
let current = null;
let cameFrom = null;
const timings = [];

try {
    index = indexGraph(await loadGraph());
    viz = await Viz.instance();
} catch (error) {
    canvas.innerHTML = `<div class="message">Could not load the graph: ${escapeHtml(error.message)}</div>`;
    throw error;
}

setUpControls();
window.addEventListener('hashchange', render);
document.addEventListener('keydown', (event) => {
    if (event.target.closest('input, select, textarea') || event.metaKey || event.ctrlKey || event.altKey) {
        return;
    }

    if (event.key === 'Escape' || event.key === 'Backspace') {
        event.preventDefault();
        goUp();
    }
});
render();

window.puml = { index, timings, renderView, benchmark, dot: (rankdir = 'TB') => toDot(current.projection, current.edges.shown, rankdir) };

async function loadGraph() {
    const embedded = document.getElementById('graph').textContent.trim();

    if (embedded) {
        return JSON.parse(embedded);
    }

    const source = new URLSearchParams(location.search).get('graph');

    if (!source) {
        throw new Error('open this page with ?graph=path/to/graph.json, or build it with puml-extract --html');
    }

    const response = await fetch(source);

    if (!response.ok) {
        throw new Error(`${source}: ${response.status}`);
    }

    return response.json();
}

function readView() {
    const params = new URLSearchParams(location.hash.slice(1));
    const level = Number(params.get('level')) || 1;

    return {
        level: Math.min(Math.max(level, 1), 3),
        focus: level > 1 ? params.get('focus') || null : null,
        vendor: params.get('vendor') !== '0',
        budget: Number(params.get('budget')) || null,
        edgeBudget: params.get('edges') === 'all' ? Infinity : Number(params.get('edges')) || DEFAULT_EDGE_BUDGET,
        kinds: params.get('kinds') ? params.get('kinds').split(',').filter(Boolean) : EDGE_KINDS,
        edge: params.has('from') && params.has('to') ? { from: params.get('from'), to: params.get('to') } : null,
    };
}

function navigate(changes) {
    const leavesEdge = ('level' in changes || 'focus' in changes) && !('edge' in changes);
    const view = { ...readView(), ...changes, ...(leavesEdge ? { edge: null } : {}) };
    const params = new URLSearchParams();

    params.set('level', view.level);

    if (view.level > 1 && view.focus) {
        params.set('focus', view.focus);
    }

    if (view.edge) {
        params.set('from', view.edge.from);
        params.set('to', view.edge.to);
    }

    if (!view.vendor) {
        params.set('vendor', '0');
    }

    if (view.budget) {
        params.set('budget', view.budget);
    }

    if (view.edgeBudget !== DEFAULT_EDGE_BUDGET) {
        params.set('edges', view.edgeBudget === Infinity ? 'all' : view.edgeBudget);
    }

    if (view.kinds.length !== EDGE_KINDS.length) {
        params.set('kinds', view.kinds.join(','));
    }

    location.hash = params.toString();
}

function render() {
    const timing = renderView(readView());

    timings.push(timing);
}

function renderView(view) {
    const started = performance.now();
    const projection = project(index, view);
    const edges = visibleEdges(projection.edges, view.edgeBudget);
    const projected = performance.now();
    const svg = layout(projection, edges.shown);
    const laidOut = performance.now();

    mount(svg, projection, edges);
    canvas.getBoundingClientRect();

    const mounted = performance.now();

    const timing = {
        level: projection.level,
        focus: projection.focus,
        nodes: projection.nodes.length,
        edges: projection.edges.length,
        shownEdges: edges.shown.length,
        project: projected - started,
        layout: laidOut - projected,
        mount: mounted - laidOut,
        total: mounted - started,
    };

    current = { view, projection, edges };
    updateChrome(view, projection, edges, timing);
    showSummary(projection, edges);

    const previous = projection.nodes.find((node) => node.id === cameFrom);
    cameFrom = null;

    if (previous) {
        selectNode(previous);
    }

    return timing;
}

function goUp() {
    const up = parentView(index, current.view);

    if (!up) {
        return;
    }

    cameFrom = current.view.edge ? null : current.projection.focus;
    navigate({ level: up.level, focus: up.focus, edge: up.edge ?? null });
}

function benchmark(views, repeat = 5) {
    const saved = current.view;

    const results = views.map((view) => {
        const runs = Array.from({ length: repeat }, () => renderView({ ...readView(), focus: null, ...view }));
        const totals = runs.map((run) => run.total).sort((a, b) => a - b);

        return { ...runs[0], median: totals[Math.floor(totals.length / 2)], max: totals.at(-1) };
    });

    renderView(saved);

    return results;
}

function layout(projection, edges) {
    const preferred = projection.edge ? 'LR' : projection.level === 3 ? 'BT' : 'TB';
    const layered = viz.renderSVGElement(toDot(projection, edges, preferred));

    if (fitScale(layered) >= 1) {
        return layered;
    }

    const sideways = viz.renderSVGElement(toDot(projection, edges, preferred === 'LR' ? 'TB' : 'LR'));
    const best = fitScale(sideways) > fitScale(layered) ? sideways : layered;

    if (fitScale(best) >= LEGIBLE_SCALE) {
        return best;
    }

    const spread = viz.renderSVGElement(toDot(projection, edges, 'TB', { layered: false }), { engine: 'neato' });

    return fitScale(spread) > fitScale(best) ? spread : best;
}

function fitScale(svg) {
    const { width, height } = svg.viewBox.baseVal;

    return Math.min(canvas.clientWidth / width, canvas.clientHeight / height);
}

function toDot(projection, edges, rankdir, { layered = true } = {}) {
    const ids = new Map(projection.nodes.map((node, position) => [node.id, `n${position}`]));

    const lines = [
        'digraph puml {',
        `graph [rankdir=${rankdir}, nodesep=${rankdir === 'LR' ? 0.12 : 0.25}, ranksep=0.55, overlap=false, splines=true, sep="+6", bgcolor="transparent", fontname="Helvetica"];`,
        'node [fontname="Helvetica", fontsize=11, shape=box, style="rounded,filled", fillcolor="#ffffff", color="#7b8794", margin="0.12,0.05"];',
        'edge [fontname="Helvetica", fontsize=9, color="#9aa5b1", fontcolor="#52606d", arrowsize=0.7];',
    ];

    const declare = (node) => `${ids.get(node.id)} [id="${ids.get(node.id)}", ${nodeAttributes(node, projection, layered)}];`;

    if (projection.edge && layered) {
        for (const side of ['from', 'to']) {
            lines.push(
                `subgraph cluster_${side} { label=${dotString(projection.edge[side])}; style="rounded,dashed"; color="#1f6feb"; fontcolor="#1f6feb"; fontsize=11;`,
                ...projection.nodes.filter((node) => node.side === side).map(declare),
                '}',
            );
        }

        lines.push(...projection.nodes.filter((node) => !node.side).map(declare));
    } else {
        lines.push(...projection.nodes.map(declare));
    }

    edges.forEach((edge, position) => {
        lines.push(`${ids.get(edge.from)} -> ${ids.get(edge.to)} [id="e${position}", ${edgeAttributes(edge, projection)}];`);
    });

    lines.push('}');

    return lines.join('\n');
}

function nodeAttributes(node, projection, layered) {
    const focus = projection.focus;
    const attributes = {
        tooltip: dotTooltip(nodeTooltip(node)),
    };

    if (node.kind === 'uml') {
        Object.assign(attributes, { shape: 'plain', style: '""', label: `<${umlLabel(index.classes.get(node.id), node)}>` });

        return formatAttributes(attributes);
    }

    const cls = index.classes.get(node.id);

    const styles = {
        module: { fillcolor: '"#e8f1fd"', color: '"#1f6feb"' },
        package: { shape: 'folder', style: 'filled', fillcolor: '"#f1f3f5"' },
        namespace: { shape: 'tab', style: 'filled', fillcolor: '"#e8f1fd"', color: '"#1f6feb"' },
        class: { fillcolor: node.focus ? '"#e8f1fd"' : cls?.external ? '"#f1f3f5"' : '"#ffffff"' },
        other: { shape: 'note', style: '"filled,dashed"', fillcolor: '"#fafafa"' },
    };

    Object.assign(attributes, styles[node.kind], { label: `<${simpleLabel(node, cls, projection, layered)}>` });

    if (node.focus) {
        attributes.penwidth = 2;
    }

    if (node.cycle) {
        Object.assign(attributes, { color: '"#d64545"', penwidth: 2 });
    }

    return formatAttributes(attributes);
}

function simpleLabel(node, cls, projection, layered) {
    const focus = projection.focus;
    const small = (text) => (text ? `<BR/><FONT POINT-SIZE="8" COLOR="#52606d">${htmlLabel(text)}</FONT>` : '');
    const contents = layered && projection.level === 2 && !projection.edge && node.focus ? small(contentsPreview(node.id)) : '';

    switch (node.kind) {
        case 'module':
            return `<B>${htmlLabel(node.label)}</B>${small(classCount(node))}${contents}`;
        case 'package':
            return `${htmlLabel(node.label)}${small(`${node.classes.length} used`)}`;
        case 'namespace':
            return `<B>${htmlLabel(`${relativeTo(node.label, focus)}\\`)}</B>${small(classCount(node))}${contents}`;
        case 'other':
            return `${htmlLabel(node.label)}${small(`${classCount(node)}, click to list`)}`;
        default:
            return `${stereotype(cls)}<B>${htmlLabel(node.label)}</B>${small(classContext(cls, focus))}`;
    }
}

function contentsPreview(namespace) {
    const entry = index.namespaces.get(namespace);

    if (!entry) {
        return '';
    }

    const names = [
        ...[...entry.children].sort().map((child) => `${child.slice(namespace.length + 1)}\\`),
        ...entry.classes.map((id) => index.classes.get(id).name).sort(),
    ];

    const shown = names.slice(0, CONTENTS_LIMIT).join(' · ');

    return names.length > CONTENTS_LIMIT ? `${shown} · +${names.length - CONTENTS_LIMIT}` : shown;
}

function classCount(node) {
    return node.classes.length === 1 ? '1 class' : `${node.classes.length} classes`;
}

function classContext(cls, focus) {
    if (!cls) {
        return '';
    }

    if (focus && cls.namespace === focus) {
        return '';
    }

    return relativeTo(cls.namespace, focus) || cls.package || '';
}

function relativeTo(namespace, focus) {
    return focus && namespace.startsWith(`${focus}\\`) ? `…${namespace.slice(focus.length)}` : namespace;
}

function umlLabel(cls, node) {
    const color = node.cycle ? '#d64545' : '#1f6feb';
    const name = cls.abstract ? `<I>${htmlLabel(cls.name)}</I>` : htmlLabel(cls.name);

    const fields = [
        ...(cls.cases ?? []).map((name) => `case ${name}`),
        ...(cls.constants ?? []).map((name) => `+${name}`),
        ...(cls.properties ?? []).map(formatProperty),
    ];

    const operations = (cls.methods ?? []).map(formatMethod);

    const header = `<TR><TD BGCOLOR="#e8f1fd" CELLPADDING="5">${stereotype(cls)}<B>${name}</B><BR/><FONT POINT-SIZE="8" COLOR="#52606d">${htmlLabel(cls.namespace)}</FONT></TD></TR>`;

    return `<TABLE BORDER="${node.cycle ? 2 : 1}" CELLBORDER="0" CELLSPACING="0" CELLPADDING="3" COLOR="${color}" BGCOLOR="#ffffff">${header}${compartment(fields)}${compartment(operations)}</TABLE>`;
}

function compartment(lines) {
    const shown = lines.slice(0, MEMBER_LIMIT).map((line) => htmlLabel(truncate(shortTypes(line))));

    if (lines.length > MEMBER_LIMIT) {
        shown.push(`… ${lines.length - MEMBER_LIMIT} more (click the class)`);
    }

    const text = shown.length === 0 ? ' ' : shown.map((line) => `${line}<BR ALIGN="LEFT"/>`).join('');

    return `<TR><TD ALIGN="LEFT" BALIGN="LEFT" SIDES="T" BORDER="1"><FONT FACE="Menlo" POINT-SIZE="9">${text}</FONT></TD></TR>`;
}

function stereotype(cls) {
    if (!cls || cls.kind === 'class') {
        return '';
    }

    return `<FONT POINT-SIZE="8">«${cls.kind}»</FONT><BR/>`;
}

function edgeAttributes(edge, projection) {
    const kinds = Object.keys(edge.kinds);
    const only = kinds.length === 1 ? kinds[0] : null;
    const classLevel = projection.level === 3 || projection.edge;

    const attributes = {
        tooltip: dotTooltip(edgeTooltip(edge)),
        label: dotString(classLevel ? describeKinds(edge) : String(edge.weight)),
        penwidth: Math.min(1 + Math.log2(edge.weight) * 0.6, 5).toFixed(2),
    };

    if (only === 'extends' || only === 'implements') {
        attributes.arrowhead = 'empty';
        attributes.arrowsize = 1;
    }

    if (only === 'implements' || only === 'uses-trait') {
        attributes.style = 'dashed';
    }

    if (edge.cycle) {
        Object.assign(attributes, { color: '"#d64545"', fontcolor: '"#b42318"' });
    }

    if (edge.violations) {
        Object.assign(attributes, { color: '"#8250df"', fontcolor: '"#6639ba"', label: dotString(`${classLevel ? describeKinds(edge) : edge.weight} ✗${edge.violations}`) });
    }

    return formatAttributes(attributes);
}

function describeKinds(edge) {
    return Object.entries(edge.kinds)
        .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
        .map(([kind, count]) => (count > 1 ? `${kind} ×${count}` : kind))
        .join(', ');
}

function edgeTooltip(edge) {
    const pairs = classPairs(edge);
    const name = (id) => index.classes.get(id)?.name ?? id;

    const lines = pairs.slice(0, TOOLTIP_PAIRS).map((pair) => `${name(pair.from)} → ${name(pair.to)}  ${describeKinds(pair)}`);

    if (pairs.length > TOOLTIP_PAIRS) {
        lines.push(`… ${pairs.length - TOOLTIP_PAIRS} more`);
    }

    const broken = edge.violations ? `✗ ${edge.violations} class edge${edge.violations === 1 ? '' : 's'} break${edge.violations === 1 ? 's' : ''} an architecture rule\n` : '';

    return `${broken}${edge.weight} × ${describeKinds(edge)}\n${lines.join('\n')}\nclick for evidence, double-click to open these classes`;
}

function classPairs(edge) {
    const pairs = new Map();

    for (const underlying of edge.edges) {
        const key = `${underlying.from}\u0000${underlying.to}`;

        if (!pairs.has(key)) {
            pairs.set(key, { from: underlying.from, to: underlying.to, weight: 0, kinds: {}, edges: [] });
        }

        const pair = pairs.get(key);
        pair.weight++;
        pair.kinds[underlying.kind] = (pair.kinds[underlying.kind] ?? 0) + 1;
        pair.edges.push(underlying);
    }

    return [...pairs.values()].sort((a, b) => b.weight - a.weight || a.from.localeCompare(b.from) || a.to.localeCompare(b.to));
}

function nodeTooltip(node) {
    if (node.kind === 'other') {
        return `${node.members.length} nodes merged to stay within the budget`;
    }

    if (node.kind === 'uml' || index.classes.get(node.id)?.external) {
        return node.id;
    }

    return `${node.id}\ndouble-click to open`;
}

function formatAttributes(attributes) {
    return Object.entries(attributes)
        .map(([key, value]) => `${key}=${value}`)
        .join(', ');
}

function dotString(text) {
    return `"${text.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

// Graphviz expands \N, \E and friends in a tooltip once more than in a label, so App\Enums would show as
// "Appnums" unless each backslash is doubled twice.
function dotTooltip(text) {
    return dotString(text.replace(/\\/g, '\\\\'));
}

function escapeHtml(text) {
    return String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

// Graphviz expands escapes such as \N and \E even inside HTML labels, so App\Events would lose its "\E".
function htmlLabel(text) {
    return escapeHtml(text).replace(/\\/g, '\\\\');
}

function truncate(text) {
    return text.length > LINE_LIMIT ? `${text.slice(0, LINE_LIMIT - 1)}…` : text;
}

function mount(svg, projection, edges) {
    svg.removeAttribute('width');
    svg.removeAttribute('height');
    canvas.replaceChildren(svg);
    enablePanZoom(svg);

    const targetOf = (event) => {
        const target = event.target.closest('g.node, g.edge');

        if (!target || svg.dataset.dragged === '1') {
            return null;
        }

        const position = Number(target.id.slice(1));

        return target.id.startsWith('n') ? { node: projection.nodes[position] } : { edge: edges.shown[position] };
    };

    svg.addEventListener('click', (event) => {
        if (svg.dataset.dragged === '1') {
            return;
        }

        const target = targetOf(event);

        if (!target) {
            clearSelection();
            showSummary(projection, edges);
        } else if (target.node) {
            selectNode(target.node);
        } else {
            selectEdge(target.edge);
        }
    });

    svg.addEventListener('dblclick', (event) => {
        const target = targetOf(event);

        if (target?.node) {
            openNode(target.node, projection);
        } else if (target?.edge) {
            openEdge(target.edge);
        }
    });
}

function selectNode(node) {
    const touching = current.projection.edges.filter((edge) => edge.from === node.id || edge.to === node.id);

    highlight(new Set([node.id, ...touching.flatMap((edge) => [edge.from, edge.to])]), new Set(touching), node.id);
    showNode(node, current.projection);
}

function selectEdge(edge) {
    highlight(new Set([edge.from, edge.to]), new Set([edge]), null);
    showEvidence(edge);
}

function highlight(nodeIds, shownEdges, selected) {
    const svg = canvas.querySelector('svg');

    current.projection.nodes.forEach((node, position) => {
        const element = svg.getElementById(`n${position}`);

        element?.classList.toggle('dim', !nodeIds.has(node.id));
        element?.classList.toggle('selected', node.id === selected);
    });

    current.edges.shown.forEach((edge, position) => {
        svg.getElementById(`e${position}`)?.classList.toggle('dim', !shownEdges.has(edge));
    });
}

function clearSelection() {
    for (const element of canvas.querySelectorAll('.dim, .selected')) {
        element.classList.remove('dim', 'selected');
    }
}

function enablePanZoom(svg) {
    const box = svg.viewBox.baseVal;
    let drag = null;

    svg.addEventListener(
        'wheel',
        (event) => {
            event.preventDefault();

            const scale = Math.exp(event.deltaY * 0.0015);
            const matrix = svg.getScreenCTM().inverse();
            const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix);

            box.x = point.x - (point.x - box.x) * scale;
            box.y = point.y - (point.y - box.y) * scale;
            box.width *= scale;
            box.height *= scale;
        },
        { passive: false },
    );

    svg.addEventListener('pointerdown', (event) => {
        drag = { x: event.clientX, y: event.clientY, moved: false };
        svg.dataset.dragged = '0';
    });

    svg.addEventListener('pointermove', (event) => {
        if (!drag) {
            return;
        }

        const dx = event.clientX - drag.x;
        const dy = event.clientY - drag.y;

        if (!drag.moved && Math.hypot(dx, dy) < 4) {
            return;
        }

        const scale = svg.getScreenCTM().a;

        drag.moved = true;
        svg.dataset.dragged = '1';
        box.x -= dx / scale;
        box.y -= dy / scale;
        drag.x = event.clientX;
        drag.y = event.clientY;
    });

    svg.addEventListener('pointerup', () => {
        drag = null;
    });
}

function openNode(node, projection) {
    const cls = index.classes.get(node.id);

    switch (node.kind) {
        case 'module':
        case 'package':
        case 'namespace':
            return navigate({ level: 2, focus: node.id });
        case 'other':
            return showOther(node, projection);
        case 'uml':
            return showClass(cls);
        case 'class':
            return cls.external ? showClass(cls) : navigate({ level: 3, focus: cls.id });
    }
}

function openEdge(edge) {
    if (current.projection.edge) {
        return;
    }

    navigate({ edge: { from: edge.from, to: edge.to } });
}

function setUpControls() {
    document.getElementById('title').textContent = index.graph.project;

    for (const button of document.querySelectorAll('#levels button')) {
        button.addEventListener('click', () => switchLevel(Number(button.dataset.level)));
    }

    const kinds = document.getElementById('kinds');

    kinds.innerHTML = EDGE_KINDS.map((kind) => `<label><input type="checkbox" value="${kind}"> ${kind}</label>`).join(' ');

    kinds.addEventListener('change', () => {
        const checked = [...kinds.querySelectorAll('input:checked')].map((input) => input.value);

        navigate({ kinds: checked });
    });

    document.getElementById('vendor').addEventListener('change', (event) => navigate({ vendor: event.target.checked }));

    const editor = document.getElementById('editor');

    editor.disabled = !index.graph.path;
    editor.value = index.graph.path ? (localStorage.getItem('puml.editor') ?? '') : '';
    editor.title = index.graph.path ? `Open file:line links in an editor, under ${index.graph.path}` : 'This graph was extracted without its project path';

    editor.addEventListener('change', () => {
        localStorage.setItem('puml.editor', editor.value);
        showSummary(current.projection, current.edges);
    });

    document.getElementById('budget').addEventListener('change', (event) => {
        navigate({ budget: Number(event.target.value) ? Math.max(5, Number(event.target.value)) : null });
    });

    const targets = new Map();

    for (const cls of index.graph.classes) {
        const module = index.modules.get(cls.id);

        targets.set(module.id, module.kind);

        if (!cls.external) {
            const segments = cls.namespace.split('\\');

            for (let length = 1; length <= segments.length; length++) {
                const namespace = segments.slice(0, length).join('\\');

                if (namespace && !targets.has(namespace)) {
                    targets.set(namespace, 'namespace');
                }
            }

            targets.set(cls.id, cls.kind);
        }
    }

    document.getElementById('targets').innerHTML = [...targets]
        .sort((a, b) => a[0].localeCompare(b[0]))
        .map(([id, kind]) => `<option value="${escapeHtml(id)}">${kind}</option>`)
        .join('');

    const search = document.getElementById('search');

    search.addEventListener('change', () => {
        const id = search.value.trim();
        const cls = index.classes.get(id);

        if (cls && !cls.external) {
            navigate({ level: 3, focus: id });
        } else if (targets.has(id)) {
            navigate({ level: 2, focus: id });
        }

        search.value = '';
    });
}

function switchLevel(level) {
    const { focus } = current.projection;

    if (level === 1) {
        return navigate({ level, focus: null });
    }

    const cls = index.classes.get(focus);

    if (level === 2) {
        return navigate({ level, focus: cls && !cls.external ? cls.namespace : focus });
    }

    return navigate({ level, focus: cls ? focus : centralClass(focus) });
}

function centralClass(namespace) {
    const degree = (cls) => (index.outgoing.get(cls.id)?.length ?? 0) + (index.incoming.get(cls.id)?.length ?? 0);

    const candidates = index.graph.classes.filter(
        (cls) => !cls.external && namespace && (cls.namespace === namespace || cls.namespace.startsWith(`${namespace}\\`)),
    );

    return candidates.sort((a, b) => degree(b) - degree(a) || a.id.localeCompare(b.id))[0]?.id ?? null;
}

function updateChrome(view, projection, edges, timing) {
    for (const button of document.querySelectorAll('#levels button')) {
        button.classList.toggle('active', Number(button.dataset.level) === projection.level);
    }

    for (const input of document.querySelectorAll('#kinds input')) {
        input.checked = view.kinds.includes(input.value);
    }

    document.getElementById('vendor').checked = view.vendor;
    document.getElementById('budget').value = projection.budget;
    document.getElementById('path').innerHTML = breadcrumb(projection);

    for (const link of document.querySelectorAll('#path a[data-view]')) {
        link.addEventListener('click', () => navigate(JSON.parse(link.dataset.view)));
    }

    document.querySelector('#path a[data-up]')?.addEventListener('click', goUp);

    const over = projection.nodes.length > projection.budget;
    const broken = projection.edges.reduce((sum, edge) => sum + edge.violations, 0);

    document.getElementById('stats').innerHTML =
        `<span class="${over ? 'warn' : ''}">${projection.nodes.length}/${projection.budget} nodes</span> · ` +
        `${edges.shown.length}/${projection.edges.length} edges drawn` +
        (projection.merged.length ? ` · ${projection.merged.length} merged into "other"` : '') +
        (broken ? ` · <span class="violation">✗ ${broken} rule violation${broken === 1 ? '' : 's'} in view</span>` : '') +
        ` · projection ${timing.project.toFixed(1)} ms, layout ${timing.layout.toFixed(1)} ms, ` +
        `total ${timing.total.toFixed(1)} ms`;
}

function breadcrumb(projection) {
    const link = (label, view) => `<a data-view='${escapeHtml(JSON.stringify(view))}'>${escapeHtml(label)}</a>`;
    const parts = [link('All modules', { level: 1, focus: null })];
    const focus = projection.focus;

    if (projection.level > 1 && focus) {
        const cls = index.classes.get(focus);
        const namespace = cls ? cls.namespace : focus;

        if (namespace.includes('/')) {
            parts.push(link(namespace, { level: 2, focus: namespace }));
        } else {
            const segments = namespace.split('\\');

            for (let length = 1; length <= segments.length; length++) {
                const prefix = segments.slice(0, length).join('\\');

                parts.push(link(segments[length - 1], { level: 2, focus: prefix }));
            }
        }

        if (cls) {
            parts.push(`<b>${escapeHtml(cls.name)}</b>`);
        }
    }

    if (projection.edge) {
        parts.push(`<b>${escapeHtml(projection.edge.from)} → ${escapeHtml(projection.edge.to)}</b> (${projection.edge.weight} class edges)`);
    }

    const up = projection.level > 1 || projection.edge ? '<a data-up title="Esc or Backspace">↑ up</a> · ' : '';

    return `${up}${projection.edge ? 'Edge' : LEVEL_NAMES[projection.level]}: ${parts.join(' › ')}`;
}

function classLink(id, text = id) {
    const cls = index.classes.get(id);
    const label = escapeHtml(text);

    if (!cls || cls.external) {
        return `<code class="muted" title="${escapeHtml(id)}">${label}</code>`;
    }

    return `<a data-focus="${escapeHtml(id)}" title="${escapeHtml(id)}"><code>${label}</code></a>`;
}

function namespaceLink(id, text = id) {
    return `<a data-namespace="${escapeHtml(id)}" title="${escapeHtml(id)}"><code>${escapeHtml(text)}</code></a>`;
}

function sourceLink(file, line) {
    const text = `<code>${escapeHtml(file)}:${line}</code>`;
    const editor = localStorage.getItem('puml.editor');
    const path = index.graph.path;

    if (!editor || !path) {
        return text;
    }

    const absolute = `${path}/${file}`;

    const urls = {
        vscode: `vscode://file${encodeURI(absolute)}:${line}`,
        cursor: `cursor://file${encodeURI(absolute)}:${line}`,
        phpstorm: `phpstorm://open?file=${encodeURIComponent(absolute)}&line=${line}`,
    };

    return `<a href="${escapeHtml(urls[editor])}" title="open in ${escapeHtml(editor)}">${text}</a>`;
}

function setPanel(html) {
    panel.innerHTML = html;

    for (const link of panel.querySelectorAll('a[data-focus]')) {
        link.addEventListener('click', () => navigate({ level: 3, focus: link.dataset.focus }));
    }

    for (const link of panel.querySelectorAll('a[data-hidden]')) {
        link.addEventListener('click', () => selectEdge(current.edges.hidden[Number(link.dataset.hidden)]));
    }

    for (const link of panel.querySelectorAll('a[data-all-edges]')) {
        link.addEventListener('click', () => navigate({ edgeBudget: Infinity }));
    }

    for (const link of panel.querySelectorAll('a[data-namespace]')) {
        link.addEventListener('click', () => navigate({ level: 2, focus: link.dataset.namespace }));
    }

    for (const link of panel.querySelectorAll('a[data-edge]')) {
        link.addEventListener('click', () => selectEdge(current.projection.edges[Number(link.dataset.edge)]));
    }

    for (const link of panel.querySelectorAll('a[data-open-edge]')) {
        link.addEventListener('click', () => openEdge(current.projection.edges[Number(link.dataset.openEdge)]));
    }
}

function showSummary(projection, edges) {
    const rules = {
        1: 'Each node is a module (PSR-4 root plus one namespace segment) or a whole vendor package.',
        2: 'The classes declared directly in the focused namespace, and one node per child namespace. Neighbours of the same module are shown as siblings, others as their module.',
        3: 'The focused class with its members, and every class it shares an edge with.',
    };

    const rule = projection.edge
        ? `The ${projection.edge.weight} class edges behind ${escapeHtml(projection.edge.from)} → ${escapeHtml(projection.edge.to)}, and the classes at both ends.`
        : rules[projection.level];

    const cycles = projection.nodes.filter((node) => node.cycle);
    const unresolved = index.graph.unresolved;

    setPanel(
        `<h2>${LEVEL_NAMES[projection.level]}${projection.focus ? `: ${escapeHtml(projection.focus)}` : ''}</h2>` +
            `<p class="muted">${rule} Click a node or edge to select it, double-click to open it. Esc goes up.</p>` +
            architecture(projection) +
            `<h3>Cycles</h3>` +
            (cycles.length
                ? `<ul>${cycles.map((node) => `<li>${escapeHtml(node.id)}</li>`).join('')}</ul>`
                : '<p class="muted">No dependency cycles in this view.</p>') +
            (projection.level === 1 && !projection.edge ? moduleCycleList() : '') +
            hiddenEdges(edges.hidden) +
            (projection.level === 3 && index.classes.get(projection.focus) ? classDetails(index.classes.get(projection.focus)) : '') +
            `<h3>Graph</h3><p class="muted">${index.graph.generator}<br>${index.graph.stats.classes} classes, ` +
            `${index.graph.stats.edges} edges, extracted in ${index.graph.stats.extract_ms} ms</p>` +
            `<details><summary>${unresolved.length} unresolved</summary><ul>` +
            unresolved.map((note) => `<li><span class="kind">${escapeHtml(note.kind)}</span>${escapeHtml(note.detail ?? '')} ${sourceLink(note.file, note.line)}</li>`).join('') +
            '</ul></details>',
    );
}

function architecture(projection) {
    const broken = projection.edges.filter((edge) => edge.violations);
    const total = broken.reduce((sum, edge) => sum + edge.violations, 0);
    const scope = projection.level === 1 && !projection.edge ? 'in the whole graph' : 'in this view';
    const { rules_apply: rulesApply, rules_scope: rulesScope, rules } = index.graph.verdict;

    if (!rulesApply) {
        return `<h3>Rules</h3><p class="muted">Not checked. ${escapeHtml(rulesScope)}</p>`;
    }

    if (total === 0) {
        return `<h3>Rules</h3><p class="muted">No architecture rule is broken ${scope}. <span title="${escapeHtml(rules.map((rule) => `${rule.id}: ${rule.why}`).join('\n'))}">${rules.length} rules checked.</span></p>`;
    }

    const byRule = new Map();

    for (const edge of broken) {
        for (const underlying of edge.edges) {
            const rule = index.violating.get(underlying);

            if (rule) {
                if (!byRule.has(rule)) {
                    byRule.set(rule, []);
                }

                byRule.get(rule).push({ underlying, edge });
            }
        }
    }

    const sections = [...byRule]
        .sort((a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0]))
        .map(([rule, hits]) => {
            const rows = hits
                .sort((a, b) => a.underlying.from.localeCompare(b.underlying.from) || a.underlying.to.localeCompare(b.underlying.to))
                .map(({ underlying, edge }) => `<li>${violationRow(underlying)} <a data-edge="${projection.edges.indexOf(edge)}" class="muted">select edge</a></li>`);

            return `<details open><summary><span class="violation">✗ ${hits.length}</span> ${escapeHtml(rule)}</summary><p class="muted">${escapeHtml(rules.find((candidate) => candidate.id === rule).why)}</p><ul class="evidence">${rows.join('')}</ul></details>`;
        });

    return `<h3>Rules</h3><p><span class="violation">✗ ${total} class edge${total === 1 ? '' : 's'}</span> break${total === 1 ? 's' : ''} an architecture rule ${scope}. Violating edges are drawn in purple.</p>${sections.join('')}`;
}

function violationRow(edge) {
    const via = edge.label ? ` <span class="muted">via ${escapeHtml(edge.label)}</span>` : '';
    const places = edge.evidence.map((place) => sourceLink(place.file, place.line)).join(' ');

    return `${classLink(edge.from, index.classes.get(edge.from).name)} → ${classLink(edge.to, index.classes.get(edge.to).name)} <span class="muted">${edge.kind}</span>${via} ${places}`;
}

function moduleCycleList() {
    const { cycles, cycle_rule: cycleRule } = index.graph.verdict;

    if (cycles.length === 0) {
        return '';
    }

    const count = (number, noun) => `${number} ${noun}${number === 1 ? '' : noun.endsWith('s') ? 'es' : 's'}`;

    const items = cycles.map((cycle, position) => {
        const lines = [`<b>Cycle ${position + 1}</b>: ${cycle.modules.map((id) => namespaceLink(id)).join(', ')}`];

        if (cycle.folds.length > 0) {
            const groups = new Map();

            for (const fold of cycle.folds) {
                const key = `${fold.into}\u0000${fold.module}\u0000${fold.kind}`;
                const name = fold.class.startsWith(`${fold.module}\\`) ? fold.class.slice(fold.module.length + 1) : fold.class;

                groups.set(key, [...(groups.get(key) ?? []), name]);
            }

            const parts = [...groups].map(([key, names]) => {
                const [into, module, kind] = key.split('\u0000');
                const listed = cycle.folds.length <= 20 ? `: ${names.map((name) => `<code>${escapeHtml(name)}</code>`).join(', ')}` : '';

                return `${namespaceLink(into)} owns ${count(names.length, kind)} from ${namespaceLink(module)}${listed}`;
            });

            lines.push(`<span class="muted">Parts of another module, counted there (a trait, interface or abstract class used by one other module only):</span>`);
            lines.push(`<ul class="evidence">${parts.map((part) => `<li>${part}</li>`).join('')}</ul>`);

            if (cycle.remaining.length < cycle.modules.length) {
                lines.push(`<span class="muted">Counted that way, still in a cycle:</span> ${cycle.remaining.map((id) => namespaceLink(id)).join(', ')}`);
            }
        }

        const cutLabel = cycle.exact ? 'Cheapest cut' : 'A cut that breaks it (too many modules to find the cheapest)';
        const cost = `${count(cycle.cost.class_edges, 'class edge')} in ${count(cycle.cost.files, 'file')}, fixes ${count(cycle.cost.violations, 'rule violation')}`;
        const shown = cycle.exact ? cycle.cut : cycle.cut.slice(0, 12);

        const rows = shown.map((edge) => {
            const underlying = cycle.exact ? (edge.edges ?? []).slice(0, 10).map((position) => `<li>${violationRow(index.graph.edges[position])}</li>`) : [];

            if (cycle.exact && (edge.edges ?? []).length > 10) {
                underlying.push(`<li class="muted">… ${edge.edges.length - 10} more</li>`);
            }

            return `<li>${namespaceLink(edge.from)} → ${namespaceLink(edge.to)} <span class="muted">${edge.weight}</span>${underlying.length ? `<ul class="evidence">${underlying.join('')}</ul>` : ''}</li>`;
        });

        if (shown.length < cycle.cut.length) {
            rows.push(`<li class="muted">… ${cycle.cut.length - shown.length} more edges in the cut</li>`);
        }

        lines.push(`<span class="muted">${cutLabel}: ${cost}</span>`);
        lines.push(`<ul class="evidence">${rows.join('')}</ul>`);

        return `<li>${lines.join('<br>')}</li>`;
    });

    return `<details open><summary>${cycles.length} module cycle${cycles.length === 1 ? '' : 's'} among app modules</summary><p class="muted">${escapeHtml(cycleRule.why)}</p><ul>${items.join('')}</ul></details>`;
}

let metricsById = null;

function metricsOf(id) {
    metricsById ??= new Map(index.graph.verdict.modules.map((module) => [module.id, module]));

    return metricsById.get(id);
}

function metricsLine(node) {
    const cls = index.classes.get(node.id);

    if (cls) {
        if (cls.external && cls.root == null) {
            return '';
        }

        const fanOut = index.graph.verdict.fan_out[cls.id] ?? 0;

        return `<p class="muted">layer <b>${cls.layer}</b> · fan-out ${fanOut} app class${fanOut === 1 ? '' : 'es'} outside its module</p>`;
    }

    const metrics = metricsOf(node.id);

    if (!metrics) {
        return '';
    }

    const sample = index.graph.classes.find((candidate) => !candidate.external && index.modules.get(candidate.id).id === node.id);
    const number = (value) => (value === null ? '–' : value.toFixed(2));
    const title = 'Ca, Ce: classes in other app modules using / used by this module. I = Ce/(Ca+Ce). A = abstract classes and interfaces / classes. D = |A+I−1|.';

    return (
        `<p class="muted" title="${escapeHtml(title)}">layer <b>${sample ? sample.layer : '?'}</b> · ` +
        `Ca ${metrics.afferent} · Ce ${metrics.efferent} · I ${number(metrics.instability)} · A ${number(metrics.abstractness)} · D ${number(metrics.distance)} · ` +
        `${metrics.vendor} vendor class${metrics.vendor === 1 ? '' : 'es'}` +
        (metrics.cycle ? ` · <span class="warn">in module cycle ${metrics.cycle}</span>` : '') +
        '</p>'
    );
}

function violationMark(count) {
    return count ? ` <span class="violation" title="class edges breaking an architecture rule">✗${count}</span>` : '';
}

function hiddenEdges(hidden) {
    if (hidden.length === 0) {
        return '';
    }

    const rows = hidden.map(
        (edge, position) => `<li><a data-hidden="${position}">${escapeHtml(edge.from)} → ${escapeHtml(edge.to)}</a> <span class="muted">${edge.weight}</span></li>`,
    );

    return (
        `<h3>Hidden edges</h3><p class="muted">${hidden.length} lighter edges are not drawn to keep the view legible. ` +
        `<a data-all-edges>Draw all edges</a></p><details><summary>list them</summary><ul>${rows.join('')}</ul></details>`
    );
}

function showClass(cls) {
    setPanel(`<h2>${escapeHtml(cls.id)}</h2>` + classDetails(cls));
}

function showNode(node, projection) {
    if (node.kind === 'other') {
        return showOther(node, projection);
    }

    const cls = index.classes.get(node.id);

    if (cls && (node.kind === 'class' || node.kind === 'uml')) {
        const open = node.kind === 'class' && !cls.external ? `<p><a data-focus="${escapeHtml(cls.id)}">Open its UML</a> <span class="muted">(or double-click)</span></p>` : '';

        return setPanel(`<h2>${escapeHtml(cls.id)}</h2>` + metricsLine(node) + open + dependencies(node, projection) + classDetails(cls));
    }

    setPanel(
        `<h2>${escapeHtml(node.id)}</h2>` +
            `<p>${node.kind}, ${classCount(node)} in this view. <a data-namespace="${escapeHtml(node.id)}">Open it</a> <span class="muted">(or double-click)</span></p>` +
            metricsLine(node) +
            contents(node.id) +
            dependencies(node, projection),
    );
}

function contents(namespace) {
    const entry = index.namespaces.get(namespace);

    if (!entry) {
        return '';
    }

    const size = (id) => {
        const child = index.namespaces.get(id);

        return child.classes.length + [...child.children].reduce((sum, grandchild) => sum + size(grandchild), 0);
    };

    const namespaces = [...entry.children].sort().map((id) => `<li>${namespaceLink(id, `${id.slice(namespace.length + 1)}\\`)} <span class="muted">${size(id)}</span></li>`);
    const classes = entry.classes
        .map((id) => index.classes.get(id))
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((cls) => `<li>${classLink(cls.id, cls.name)} <span class="muted">${cls.kind === 'class' ? '' : cls.kind}</span></li>`);

    return `<h3>Contents</h3><ul>${namespaces.join('')}${classes.join('')}</ul>`;
}

function dependencies(node, projection) {
    const list = (title, edges, other) => {
        if (edges.length === 0) {
            return '';
        }

        const rows = edges
            .sort((a, b) => b.weight - a.weight)
            .map((edge) => `<li><a data-edge="${projection.edges.indexOf(edge)}"><code>${escapeHtml(edge[other])}</code></a> <span class="muted">${edge.weight} · ${escapeHtml(describeKinds(edge))}</span>${violationMark(edge.violations)}</li>`);

        return `<h3>${title}</h3><ul>${rows.join('')}</ul>`;
    };

    return (
        list('Depends on', projection.edges.filter((edge) => edge.from === node.id), 'to') +
        list('Used by', projection.edges.filter((edge) => edge.to === node.id), 'from')
    );
}

function classDetails(cls) {
    const list = (title, items) => (items.length ? `<h3>${title}</h3><ul>${items.map((item) => `<li>${item}</li>`).join('')}</ul>` : '');
    const code = (text) => `<code>${escapeHtml(text)}</code>`;

    return (
        `<p>${cls.abstract ? 'abstract ' : ''}${cls.final ? 'final ' : ''}${cls.kind}` +
        (cls.file ? ` in ${sourceLink(cls.file, cls.line)}` : ' (not scanned)') +
        `<br><a data-namespace="${escapeHtml(cls.namespace)}">${escapeHtml(cls.namespace)}</a></p>` +
        list('Extends', cls.parent ? [classLink(cls.parent)] : []) +
        list('Implements', (cls.interfaces ?? []).map((id) => classLink(id))) +
        list('Uses', (cls.traits ?? []).map((id) => classLink(id))) +
        list(
            'Relations',
            (cls.relations ?? []).map((relation) => `${code(relation.name)} ${relation.type} ${relation.related ? classLink(relation.related) : '<span class="muted">runtime</span>'}`),
        ) +
        list('Cases', (cls.cases ?? []).map(code)) +
        list('Constants', (cls.constants ?? []).map(code)) +
        list('Properties', (cls.properties ?? []).map((property) => code(formatProperty(property)))) +
        list('Methods', (cls.methods ?? []).map((method) => `${code(formatMethod(method))} ${cls.file ? memberLink(cls.file, method.line) : ''}`))
    );
}

function memberLink(file, line) {
    const link = sourceLink(file, line);

    return link.startsWith('<a') ? link.replace(/<code>.*<\/code>/, `<code>:${line}</code>`) : `<span class="muted">:${line}</span>`;
}

function showOther(node, projection) {
    const items = node.members.map((id) =>
        index.classes.has(id) && !index.classes.get(id).external ? classLink(id) : `<a data-namespace="${escapeHtml(id)}"><code>${escapeHtml(id)}</code></a>`,
    );

    setPanel(
        `<h2>${node.members.length} merged nodes</h2>` +
            `<p class="muted">These had the fewest edges in the ${LEVEL_NAMES[projection.level].toLowerCase()} view and were merged to stay within the budget of ${current.projection.budget}. Raise the budget or focus one of them.</p>` +
            `<ul>${items.map((item) => `<li>${item}</li>`).join('')}</ul>`,
    );
}

function showEvidence(edge) {
    const pairs = classPairs(edge);
    const classes = new Set(pairs.flatMap((pair) => [pair.from, pair.to]));
    const open = current.projection.edge
        ? ''
        : `<p><a data-open-edge="${current.projection.edges.indexOf(edge)}">Open these ${classes.size} classes</a> <span class="muted">(or double-click the edge)</span></p>`;

    const rows = pairs.slice(0, EVIDENCE_LIMIT).map((pair) => {
        const name = (id) => index.classes.get(id)?.name ?? id;

        const evidence = pair.edges.map((underlying) => {
            const via = underlying.label ? ` <span class="muted">via ${escapeHtml(underlying.label)}</span>` : '';
            const places = underlying.evidence.map((place) => sourceLink(place.file, place.line)).join(' ');

            const rule = index.violating.get(underlying);
            const broken = rule ? ` <span class="violation">✗ ${escapeHtml(rule)}</span>` : '';

            return `<li><span class="kind">${underlying.kind}</span>${via} ${places}${broken}</li>`;
        });

        return (
            `<li><details${pairs.length <= 3 ? ' open' : ''}><summary>${classLink(pair.from, name(pair.from))} → ${classLink(pair.to, name(pair.to))} ` +
            `<span class="muted">${escapeHtml(describeKinds(pair))}</span>${violationMark(pair.edges.filter((underlying) => index.violating.has(underlying)).length)}</summary><ul class="evidence">${evidence.join('')}</ul></details></li>`
        );
    });

    if (pairs.length > EVIDENCE_LIMIT) {
        rows.push(`<li class="muted">… ${pairs.length - EVIDENCE_LIMIT} more class pairs</li>`);
    }

    setPanel(
        `<h2>${escapeHtml(edge.from)} → ${escapeHtml(edge.to)}</h2>` +
            `<p>${edge.weight} class edge${edge.weight === 1 ? '' : 's'} between ${pairs.length} class pair${pairs.length === 1 ? '' : 's'}: ${escapeHtml(describeKinds(edge))}` +
            (edge.cycle ? '<br><b style="color:#b42318">part of a dependency cycle</b>' : '') +
            (edge.violations ? `<br><b class="violation">✗ ${edge.violations} of them break an architecture rule</b>` : '') +
            `</p>${open}<ul>${rows.join('')}</ul>`,
    );
}
