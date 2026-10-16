# Contributing to puml

puml has two halves. A Rust extractor, `puml-extract`, uses
[mago](https://github.com/carthage-software/mago)'s crates to turn a project into a versioned JSON graph
and judges the architecture verdict. A static viewer draws that graph with Graphviz (WASM) in the
browser. The viewer is compiled into the binary, and `--html` writes both into one file.

```
extractor/   puml-extract (Rust, links mago 1.54.0)
  src/collect.rs    per-file facts: declarations, members and every resolved name
  src/assemble.rs   modules, packages, edges and the graph
  src/laravel.rs    Eloquent relations and facades
  src/verdict.rs    layers, rules, module cycles with their cheapest cut, metrics, fan-out outliers
  src/report.rs     the text of `report` and `show`
viewer/      index.html, app.js (rendering and the side panel), model.js (levels, aggregation, budgets, cycles in a view)
fixtures/    shop: a small fixture app with a hand-written expected graph
             blog: a fixture with seeded rule violations and a module cycle, with hand-written expected/violations.json
tests/       golden, invariant, drill, cycle, edge-budget and rule tests on the JSON contract (node --test)
verify/      PHP judges that compare a graph with what PHP itself reports, and a deptrac comparison
assets/      the icon and the README screenshots
```

## Building and testing

```sh
(cd extractor && cargo build --release)
(cd extractor && cargo test --release)
pnpm install && pnpm test
```

Build from inside `extractor/`: `rust-toolchain.toml` pins the toolchain there. The viewer is embedded
with `include_str!`, so rebuild after any change in `viewer/` and regenerate the HTML you are looking at.

While working on the viewer, you can also serve the repository and open
`viewer/index.html?graph=../out/app.json`, which loads the modules directly.

The golden files in `fixtures/shop/expected/` and `fixtures/blog/expected/violations.json` are written by
hand. When one fails, decide by hand which side is wrong before you touch it.

## Why the crates and not the `mago` CLI

`mago ast --json app/Models/User.php` (mago 1.54.0) prints the concrete syntax tree of one file.

- Names are not resolved: `extends Authenticatable` comes out as a `Local` identifier, and the `use`
  alias has to be applied by the caller.
- Identifiers are byte arrays.
- Comments, and so PHPDoc generics, are not in the output.
- The output is about 150 times the size of the source: 128 KB of JSON for the 840-byte `User.php`.
- Running it once per file over `laravel/framework/src` took 10.7 s.

Linking the crates gives all of this in-process. `mago-names` resolves every identifier, and
`mago-codex` scans each file into class metadata, including PHPDoc types. One pass over the 1,421
framework files takes about 0.1 s.

## How the viewer builds a view

**Level 1, modules.** A node is a module:

- a PSR-4 root from `composer.json` plus the next namespace segment, such as `App\Models` or
  `Illuminate\Database`
- or a whole vendor package, such as `laravel/framework`, collapsed into one node
- or `php` for built-in classes

Every module that declares a class is shown. A vendor package is shown only when an edge reaches it.

**Level 2, classes.** The focus is a namespace or a package, and the view walks the namespace tree one
segment at a time:

- Each class declared directly in the focus is a node.
- Each child namespace is one node, labelled with its class count and the first few names inside it.
  A chain of namespaces that hold no classes and have one child each is compacted into one node, so
  `Controllers\Admin\Blog` is a single step.
- A neighbour in the focus's own module is shown at sibling granularity: a class next to the focus is
  a class node, and anything deeper is the sibling namespace that holds it. With the focus
  `App\Http\Controllers`, a dependency on `App\Http\Requests\StoreUser` is drawn as `App\Http\Requests`.
- A neighbour in any other module is shown as its module, and only when it shares an edge with the
  focus.

**Level 3, UML.** The focus is a class, or every class in a namespace. A focused class is drawn with its
members; every class sharing an edge with the focus is drawn as a plain node.

**Aggregation.** At every level, an edge between two nodes aggregates every class edge between their
classes, and its weight is the number of class edges under it. Levels 2 and 3 only show edges that
touch the focus.

**Edge view.** The classes at both ends of an aggregated edge, grouped in one box per end, and only the
class edges between them. The URL is the base view plus `from` and `to`, for example
`#level=2&focus=App%5CHttp&from=App%5CHttp%5CControllers&to=App%5CModels`. The rest of the view state
lives in the hash too, for example `#level=2&focus=App%5CModels&budget=30&kinds=extends,relation`.

**Cycles in a view** are the strongly connected components of the projected graph (Tarjan). The module
cycles in the verdict are computed by the extractor on the full module graph.

**Node budget.** 40 nodes at levels 1 and 2, and 24 at level 3, where the focused class alone takes the
room of many plain nodes. Past the budget, the lowest-ranked nodes merge into one "N more" node. Cycles
are found before the merge, so merging cannot create one.

**Edge budget.** At most 60 edges are drawn: first the heaviest edge of each node, so no node loses all
of its edges, then cycle edges before others, heaviest first. Hidden edges are listed in the panel.

**Layout.** The viewer tries `dot` top to bottom (bottom to top at level 3), then `dot` left to right,
and if neither reaches a scale of 0.8, `neato`, without the contents lines and the edge view's end
boxes, which `neato` cannot place. Whichever layout has the larger scale wins.

## The verdict

The verdict is judged once, in `extractor/src/verdict.rs`, and written into the graph JSON, so the
viewer and `report` read the same violations, cycles and metrics and never disagree.

- A class's layer is decided by the first namespace segment under its PSR-4 root. The segment table is
  `LAYER_SEGMENTS` in `verdict.rs`.
- Providers, database and tests may depend on anything, and anything may depend on providers: the stock
  Breeze controllers reference `AppServiceProvider::HOME`. Vendor classes are never part of a rule.
- The rules are only checked when the project depends on `laravel/framework`; `laravel/framework`
  itself has an `Illuminate\Http` namespace and would fail every one of them.
- The cheapest cut is the exact minimum feedback arc set for cycles of up to 12 modules; larger cycles
  get a greedy cut labelled as such.
- Metrics follow Martin's *Agile Software Development*. Fan-out outliers are above `mean + 2 sd` and at
  least 5.

A fresh `laravel/laravel` skeleton must produce zero violations and zero cycles. `fixtures/blog` seeds
six violations and one cycle.

## Graph JSON (version 2)

```
{ version, project, path, generator, roots, packages, classes, edges, unresolved, stats, verdict }
class: { id, name, namespace, package, root, kind, external, layer, ... }
edge: { from, to, kind, label?, evidence: [{ file, line }] }
verdict: { rules_apply, rules_scope, rules, cycle_rule, layer_segments, violations: [{ rule, edge }],
           cycles, modules, fan_out, outliers, layers }
```

A violation's `edge` is an index into `edges`. The JSON is the contract under test:
`fixtures/shop/expected/` holds the hand-written expected graph and the expected projection at each
level.

Facades are resolved from `getFacadeAccessor()`: a `Foo::class` return resolves to that class, a string
key is looked up in `Application::registerCoreContainerAliases()`, real-time facades
(`Facades\App\Foo`) resolve to `App\Foo`, and global aliases such as `\Route` go through
`Facade::defaultAliases()`.

## Checking the graph against PHP

```sh
php verify/inheritance.php out/app.json path/to/app/vendor/autoload.php [--json report.json]
php verify/classes.php     out/app.json path/to/app
php verify/relations.php   out/app.json path/to/app [--json report.json]
php verify/facades.php     out/app.json path/to/app
```

| verifier | compares the graph against |
|---|---|
| `inheritance.php` | `ReflectionClass` for every class, reporting precision, recall and every mismatch for `extends`, `implements` and `uses-trait` |
| `classes.php` | the class-likes PHP's own tokenizer finds in every scanned file |
| `relations.php` | `php artisan model:show --json` for every model, by relation name, relation type and related class (run it from the app root, with a working database connection) |
| `facades.php` | the class each facade resolves to at runtime in the booted app |

The rule violations are checked against [deptrac](https://github.com/deptrac/deptrac).
`verify/deptrac-config.js` writes a deptrac configuration from the rule table in a graph's verdict, and
`verify/deptrac.js` compares deptrac's JSON report with puml's violations by file, line and class pair:

```sh
node verify/deptrac-config.js out/app.json path/to/app > /tmp/deptrac/app.php
(cd /tmp/deptrac && vendor/bin/deptrac analyse -c app.php --formatter json --output app.json --no-progress --no-cache)
node verify/deptrac.js out/app.json /tmp/deptrac/app.json path/to/app
```

On laravel.io both tools report the same 7 violations at the same lines; on `fixtures/blog` the same 6.

## Screenshots

The README screenshots are of laravel.io at 1600×1000, taken with headless Chrome from a regenerated
`out/laravelio.html` served on port 8765:

```sh
shot() { "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --hide-scrollbars \
    --window-size=1600,1000 --virtual-time-budget=8000 --screenshot="$PWD/assets/screenshots/$1.png" \
    "http://127.0.0.1:8765/out/laravelio.html#$2"; }
shot modules 'level=1'
shot http    'level=2&focus=App%5CHttp'
shot thread  'level=3&focus=App%5CModels%5CThread'
shot edge    'level=1&from=App%5CJobs&to=App%5CHttp'
```

## Releasing

CI (`.github/workflows/ci.yml`) builds the extractor and runs `cargo test` and `pnpm test` on every
push and pull request. It has no corpora, so the laravel.io and framework invariants are skipped there.

To release, bump `version` in `extractor/Cargo.toml`, commit, and push a matching tag:

```sh
git tag v0.2.0
git push origin v0.2.0
```

`.github/workflows/release.yml` fails if the tag and the crate version differ, builds the four binaries
and publishes a GitHub release with the archives, `SHA256SUMS` and generated notes. Update `VERSION` in
the README's install instructions with each release.

## Dependencies

Every dependency here was at least 7 days old when it was added:

- **pnpm** enforces this with `minimumReleaseAge: 10080` in `pnpm-workspace.yaml`.
- **Cargo** has no such setting. `Cargo.lock` was generated with `--publish-time` set 7 days back, and
  the mago crates are pinned to `=1.54.0`.
- **Composer** (2.10) has no release-age setting either. Corpora without a committed lock were resolved
  with `composer update --no-install`. Any package younger than 7 days was then pinned with `--with
  vendor/package:version` before installing.
- **GitHub Actions** are pinned to the commit of a release that was at least 7 days old.
