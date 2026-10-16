// Evaluated in the viewer page: what the current view draws, and the smallest label as rendered on screen.
(() => {
    const svg = document.querySelector('#canvas svg');

    if (!svg) {
        return { hash: location.hash, error: 'no svg' };
    }

    const scale = Math.hypot(svg.getScreenCTM().a, svg.getScreenCTM().b);
    const canvas = document.getElementById('canvas').getBoundingClientRect();
    let smallest = { px: Infinity, text: '' };

    for (const text of svg.querySelectorAll('text')) {
        const box = text.getBoundingClientRect();

        if (box.right < canvas.left || box.left > canvas.right || box.bottom < canvas.top || box.top > canvas.bottom) {
            continue;
        }

        const px = parseFloat(text.getAttribute('font-size')) * scale;

        if (px < smallest.px) {
            smallest = { px: Math.round(px * 10) / 10, text: text.textContent };
        }
    }

    return {
        hash: decodeURIComponent(location.hash),
        nodes: svg.querySelectorAll('g.node').length,
        edges: svg.querySelectorAll('g.edge').length,
        scale: Math.round(scale * 1000) / 1000,
        smallestLabelPx: smallest.px,
        smallestLabel: smallest.text,
        stats: document.getElementById('stats').textContent,
    };
})()
