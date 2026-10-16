# Judge brief

You are reviewing the architecture of two PHP projects you have never seen, using only a web-based
dependency viewer. You have no access to the source code and must not look for it.

## Rules

- The only command you may run is `node verify/browse.mjs …` from the repository root,
  always with `PUML_BROWSE_PORT={PORT} PUML_BROWSE_DIR={DIR}` in front of it. The only files you may
  read are the screenshots it writes into `{DIR}`. Do not read, grep or list any other file, do not run
  `puml-extract`, `php`, `grep`, `cat` or anything else, and do not open the JSON graph.
- Start Chrome once with `PUML_BROWSE_PORT={PORT} PUML_BROWSE_DIR={DIR} node verify/browse.mjs start`,
  and stop it when you are done.
- Commands (each prints the path of a 1280x800 screenshot; read it to see the page):
  - `open <url>`: open a page.
  - `click <label>` and `dblclick <label>`: click or double-click a node by its visible label, e.g. `click 'App\Models'`.
  - `click-edge <from label> <to label>` and `dblclick-edge …`: click or double-click an edge.
  - `link <text>`: click a link or toggle in the side panel or breadcrumb.
  - `search <name>`: type a fully qualified name into the search box and press Enter.
  - `key Escape`: press a key.
  - `panel`: print the side panel's text, as if you had scrolled through it. This costs no interaction.
  - `shot`: take a screenshot only. This costs no interaction.
- Each `click`, `dblclick`, `click-edge`, `dblclick-edge`, `link`, `search` and `key` counts as one
  interaction. `open` of the start URL, `panel` and `shot` are free.
- Before each question, `open` the start URL again, so every question starts from the opening view.
  Use the fewest interactions you can. Record each one.
- Judge only by what the viewer shows you. If you cannot read something in a screenshot, say so; do
  not guess it from the `panel` text unless the panel shows it.

## Project 1: laravel.io

Start URL: `{BASE}laravelio.html`

1. What are the top-level modules, and which hold the most classes?
2. Which modules are the HTTP entry layer, and which way do dependencies point between that layer and the models?
3. Where does `POST /forum/create-thread` go: controller → ? → model? (The route is `threads.store`, handled by a forum threads controller.)
4. Which dependencies break Laravel conventions?
5. Which violation is the worst, and why?
6. Are there dependency cycles between modules? Which modules?
7. What is the cheapest cut of the cycle that involves `App\Http` and `App\Jobs`?
8. Which module is the riskiest to change (the most depended upon)?
9. Which module depends on the most other modules' classes, and which class is the busiest?
10. Where should a newcomer start reading?
11. Do events know their listeners?
12. Which modules does nothing else in the application depend on (the leaves)?

## Project 2: a small blog

Start URL: `{BASE}blog-seeded.html`

B1. Which architecture rules are broken, and by which classes?
B2. Which module cycles exist?
B3. What is the cheapest cut of each cycle?

## Report

For each question give:

- your answer, naming modules, classes and numbers;
- the exact interactions you used, and how many;
- the path of the screenshot that shows the answer;
- your confidence (high, medium or low);
- anything that was hard to read or find.

End with a short list of what slowed you down most in the viewer.
