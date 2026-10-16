# Working on puml

## Ask the graph before reading source

```sh
B=extractor/target/release/puml-extract
L=~/src-ref/corpora/laravelio
$B report $L
$B show $L 'App\Models'
$B show $L 'App\Jobs' 'App\Http'
```

`report` answers "is this architecture conventional": rule violations with file:line, module cycles with
the cheapest cut, coupling metrics per module, fan-out outliers. `show` drills into a module, namespace,
class or edge. Rules, cycles and metrics live in `extractor/src/verdict.rs`, which writes them into the
graph as `verdict`; the viewer only reads it. Change them there, in its unit tests and in
`tests/rules.test.js`.

# Verifying puml

A change is not done until the graph is checked against PHP itself and the views have been looked at
in a browser. Passing tests do not prove a view is legible.

## Build and regenerate

```sh
(cd extractor && cargo build --release)
```

- Build from inside `extractor/`. `rust-toolchain.toml` pins 1.99.0, and a build from the repository root
  picks up the wrong rustc.
- The viewer is compiled into the binary with `include_str!`. After any change in `viewer/`, rebuild and
  regenerate `out/`, or you are checking the old viewer.

```sh
B=extractor/target/release/puml-extract
L=~/src-ref/corpora/laravelio
$B $L --out out/laravelio.json --html out/laravelio.html
$B $L/vendor/laravel/framework --out out/framework.json --html out/framework.html
$B ~/src-ref/corpora/skeleton --out out/skeleton.json --html out/skeleton.html
$B fixtures/shop --out /tmp/shop.json --html out/shop.html
```

## Tests

```sh
(cd extractor && cargo test --release)
pnpm test
```

- The invariant tests also run on `out/laravelio.json` and `out/framework.json`, but only if those files
  exist. Regenerate them first, or the large corpora are silently skipped.
- `fixtures/shop/expected/` is hand-written. When a golden fails, decide by hand which side is wrong
  before touching the expected file. Never update it just to turn the test green.
- The fixture has no nested namespaces. Drilling rules are covered by the synthetic graph in
  `tests/drill.test.js`; extend it when a rule about the namespace tree changes.
- The invariants that caught real bugs:
  - module edges sum to the class edges that cross modules
  - an edge view holds exactly the class edges of the edge it opens
  - going up always ends at level 1
  - every view stays within the node budget

## Compare the graph with PHP

```sh
L=~/src-ref/corpora/laravelio
php verify/inheritance.php out/laravelio.json $L/vendor/autoload.php --json out/laravelio.inheritance.json
php -d memory_limit=2G verify/inheritance.php out/framework.json $L/vendor/autoload.php --json out/framework.inheritance.json
php verify/classes.php out/laravelio.json $L
php verify/classes.php out/framework.json $L/vendor/laravel/framework
cd $L && DB_CONNECTION=sqlite DB_DATABASE=$PWD/database/database.sqlite \
    php ~/.t3/projects/puml/verify/relations.php ~/.t3/projects/puml/out/laravelio.json . --json ~/.t3/projects/puml/out/laravelio.relations.json
cd $L && DB_CONNECTION=sqlite DB_DATABASE=$PWD/database/database.sqlite \
    php ~/.t3/projects/puml/verify/facades.php ~/.t3/projects/puml/out/laravelio.json .
```

- The bar is precision and recall of 1.0 for `extends`, `implements` and `uses-trait` on both corpora,
  and 34/34 relations on laravel.io.
- Rule violations are checked against deptrac (installed outside the repository, e.g. `/tmp/puml-deptrac`):
  `node verify/deptrac-config.js out/laravelio.json $L > /tmp/puml-deptrac/laravelio.php`, run `vendor/bin/deptrac analyse -c
  laravelio.php --formatter json --output laravelio.json --no-progress --no-cache` there, then
  `node verify/deptrac.js out/laravelio.json /tmp/puml-deptrac/laravelio.json $L`. The bar is zero lines in
  "only puml" and "only deptrac" on laravel.io and `fixtures/blog`.
- The skeleton must report zero violations and zero cycles; `fixtures/blog` must match its
  `expected/violations.json`. Never edit that file to make a test pass; decide by hand which side is wrong.
- Any new mismatch is a bug until it is explained. The one known gap is `User::tokens()`, whose related
  class only exists at runtime.
- The framework run needs `memory_limit=2G`. `relations.php` and `facades.php` boot the app, so they need
  the sqlite connection above.

## Look at it in a browser

```sh
python3 -m http.server 8765 --bind 127.0.0.1
```

Serve the repository root and open `http://127.0.0.1:8765/out/laravelio.html`. Change a query parameter
(`?r=2`) after regenerating, so the browser does not show a cached file. The view is in the hash, so a
view can be opened directly, for example
`#level=2&focus=App%5CHttp&from=App%5CHttp%5CControllers&to=App%5CModels`.

Check at least these, and take screenshots:

- level 1, `App\Http`, `App\Http\Controllers` and an edge view on laravel.io
- `Illuminate\Database\Eloquent` and the edge view `Relations → Model` on the framework, which falls
  back to `neato`
- click selects and dims, double-click drills, Esc goes up and reselects the node you came from

Legibility regressions only show up in screenshots:

- A label line added to every node widened every box and shrank `App\Http` until it was unreadable.
- `neato` ignores clusters. In the edge view, the end boxes covered the whole drawing until they were
  dropped from the `neato` layout.

## Timings

The target is under 100 ms per view on the framework.

- The stats line under the toolbar shows projection, layout and total time for the current view.
- `window.puml.benchmark(views)` renders each view five times and returns the median and maximum,
  for example `puml.benchmark([{ level: 1, edge: null }, { level: 2, focus: 'Illuminate\\Database', edge: null }])`.
- `puml.renderView` needs a complete view, including `kinds`. To time a single view, set
  `location.hash` and read the stats line instead.
