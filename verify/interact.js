// Evaluated in the viewer page before a task's steps. Each ui.* call is one reviewer interaction: a click,
// a double-click or a key, aimed at what the reviewer sees (a node or edge by its label, a panel link by
// its text), never at internal state.
window.ui = (() => {
    const svg = () => document.querySelector('#canvas svg');
    const fire = (element, type) => element.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }));
    const words = (element) => [...element.querySelectorAll('text')].map((text) => text.textContent.trim());

    const node = (label) => {
        const nodes = [...svg().querySelectorAll('g.node')];
        const found = nodes.find((element) => words(element)[0] === label) ?? nodes.find((element) => words(element).some((word) => word === label));

        if (!found) {
            throw new Error(`no node labelled ${label}`);
        }

        return found;
    };

    const edge = (from, to) => {
        const ids = (label) => node(label).querySelector('title').textContent;
        const title = `${ids(from)}->${ids(to)}`;
        const found = [...svg().querySelectorAll('g.edge')].find((element) => element.querySelector('title').textContent === title);

        if (!found) {
            throw new Error(`no edge ${from} -> ${to}`);
        }

        return found.querySelector('path');
    };

    const link = (text) => {
        const found = [...document.querySelectorAll('#panel a, #path a, #panel summary')].find((element) => element.textContent.trim() === text)
            ?? [...document.querySelectorAll('#panel a, #path a, #panel summary')].find((element) => element.textContent.includes(text));

        if (!found) {
            throw new Error(`no link ${text}`);
        }

        return found;
    };

    const settle = () => new Promise((resolve) => setTimeout(resolve, 150));

    return {
        click: (label) => (fire(node(label).querySelector('text'), 'click'), settle()),
        dblclick: (label) => (fire(node(label).querySelector('text'), 'dblclick'), settle()),
        clickEdge: (from, to) => (fire(edge(from, to), 'click'), settle()),
        dblclickEdge: (from, to) => (fire(edge(from, to), 'dblclick'), settle()),
        link: (text) => (link(text).click(), settle()),
        search: (text) => {
            const input = document.getElementById('search');

            input.value = text;
            input.dispatchEvent(new Event('change', { bubbles: true }));

            return settle();
        },
        key: (key) => (document.body.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true })), settle()),
        nodes: () => [...svg().querySelectorAll('g.node')].map(words),
        panel: () => document.getElementById('panel').innerText,
        visible: () => `${document.getElementById('crumbs').innerText}\n${[...svg().querySelectorAll('g.node, g.edge')].map((element) => words(element).join(' ')).join('\n')}\n${document.getElementById('panel').innerText}`,
    };
})();
