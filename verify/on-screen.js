// Evaluated in the viewer page: the text a reviewer can read without scrolling or zooming. That is the
// breadcrumb and stats line, SVG labels rendered at 11 px or more inside the canvas, and the part of the
// side panel that is inside its visible box.
(() => {
    const inside = (box, frame) => box.width > 0 && box.bottom > frame.top && box.top < frame.bottom && box.right > frame.left && box.left < frame.right;
    const svg = document.querySelector('#canvas svg');
    const canvas = document.getElementById('canvas').getBoundingClientRect();
    const scale = svg ? Math.hypot(svg.getScreenCTM().a, svg.getScreenCTM().b) : 1;
    const labels = svg
        ? [...svg.querySelectorAll('text')]
              .filter((text) => parseFloat(text.getAttribute('font-size')) * scale >= 11 && inside(text.getBoundingClientRect(), canvas))
              .map((text) => text.textContent)
        : [];

    const panel = document.getElementById('panel');
    const frame = panel.getBoundingClientRect();
    const walker = document.createTreeWalker(panel, NodeFilter.SHOW_TEXT);
    const range = document.createRange();
    const lines = [];

    while (walker.nextNode()) {
        range.selectNodeContents(walker.currentNode);

        if (inside(range.getBoundingClientRect(), frame)) {
            lines.push(walker.currentNode.textContent);
        }
    }

    return `${document.getElementById('crumbs').innerText}\n${labels.join('\n')}\n${lines.join('')}`;
})()
