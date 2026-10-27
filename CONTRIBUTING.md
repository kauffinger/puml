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
  src/git.rs        the merge base, the changed files and their contents at the base
  src/diff.rs       the base graph, and what changed between it and the head
  src/diff_report.rs  the text and JSON of `diff`
  src/diff_markdown.rs  the Markdown of `diff`, for the pull request comment
viewer/      index.html, app.js (rendering and the side panel), model.js (levels, aggregation, budgets, cycles in a view)
fixtures/    shop: a small fixture app with a hand-written expected graph
             blog: a fixture with seeded rule violations and a module cycle, with hand-written expected/violations.json
             blog-pr: a pull request on blog (head/ laid over it, deleted lists removed files), with hand-written expected/diff.json
tests/       golden, invariant, drill, cycle, edge-budget, rule, diff and action tests (node --test)
verify/      PHP judges that compare a graph with what PHP itself reports, and a deptrac comparison
action/      the GitHub Action's scripts: install.sh downloads a release, report.sh runs `diff`, annotates and comments
action.yml   the GitHub Action
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

The golden files in `fixtures/shop/expected/`, `fixtures/blog/expected/violations.json` and
`fixtures/blog-pr/expected/diff.json` are written by hand. When one fails, decide by hand which side is wrong before you touch it.

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

## The diff

`puml-extract diff <dir> <base>` compares two graphs of the same project.

- **The head** is the working tree, including uncommitted and untracked files. **The base** is
  `git merge-base <base> HEAD`, the commit a pull request is compared with on GitHub.
- **Changed files** come from `git diff --cached --name-status -M` against the base, run on a copy of
  the index after `git add --all`, so untracked files and renames count and the user's index is never
  touched.
- **The base graph** reuses the head's facts for unchanged files, collects changed files from
  `git show <base>:<path>`, and assembles and judges the result like any graph. `vendor/` is read from
  disk for both sides.
- **An edge** is the same edge on both sides when its from, to, kind and label match. Static and
  signature edges ignore the label, which names the method. Evidence is never compared, so moving code
  inside a file changes nothing.
- **A rename** is a file git reports as renamed that holds exactly one removed and one added class.
- **A removed class still in use** is a class declared at the base that the head only references: it is
  external but under an app root.
- **Cycles** are compared by `remaining`, the modules still in a cycle once traits, interfaces and
  abstract classes are counted with their users, as in the report. A trait module that joins a cycle
  through its users is not a join.
- **The reading order** groups the classes of the changed files into strongly connected components
  (Tarjan) and orders the groups so a class comes after the classes of the pull request it depends on
  (Kahn).
- **The impact** is the classes outside the pull request that use a modified class, and everything that
  reaches a modified class transitively, counted per module.
- `--strict` exits 1 when the head adds a rule violation, adds a module to a cycle, or leaves a removed
  class in use.

`tests/diff.test.js` builds a git repository from `fixtures/blog` and `fixtures/blog-pr` and checks the
diff against `expected/diff.json` before and after the pull request is committed, and checks the edge
changes against fresh extractions of both sides.

## Graph JSON (version 3)

```
{ version, project, path, generator, roots, packages, classes, edges, unresolved, stats, verdict, laravel }
class: { id, name, namespace, package, root, kind, external, layer, ... }
edge: { from, to, kind, label?, evidence: [{ file, line }] }
verdict: { rules_apply, rules_scope, rules, cycle_rule, layer_segments, violations: [{ rule, edge }],
           cycles, modules, fan_out, outliers, layers }
laravel: { routes, listeners, observers, policies, bindings, schedule, middleware }, each fact with file and line
```

Route files and `bootstrap/app.php` are classes of `kind: "file"` whose id is their relative path, in
layer `entry`. Every edge of kind `route`, `middleware`, `listener`, `observer`, `policy`, `binding` or
`schedule` stands for a fact of `laravel` at the same file:line (`tests/wiring.test.js` checks both
directions). These wiring edges are left out of the rules, cycles, metrics and fan-out
(`EdgeKind::is_wiring()` in Rust, `WIRING_KINDS` in the viewer). The facts are read in
`extractor/src/routes.rs` and `wiring.rs`, and joined into edges in `wiring_graph.rs`. What the framework
does differently between versions (router verbs, observable events, policy guessing, event discovery,
default middleware aliases) is read from `vendor/laravel/framework`, with Laravel 11+ as the fallback.
`fixtures/wiring/expected/laravel.json` holds the hand-written facts of the wiring fixture.

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
php verify/routes.php      out/app.json path/to/app [--json report.json]
php verify/wiring.php      out/app.json path/to/app [--json report.json]
```

| verifier | compares the graph against |
|---|---|
| `inheritance.php` | `ReflectionClass` for every class, reporting precision, recall and every mismatch for `extends`, `implements` and `uses-trait` |
| `classes.php` | the class-likes PHP's own tokenizer finds in every scanned file |
| `relations.php` | `php artisan model:show --json` for every model, by relation name, relation type and related class (run it from the app root, with a working database connection) |
| `facades.php` | the class each facade resolves to at runtime in the booted app |
| `routes.php` | every route the booted app registers, recorded with the call stack that added it: methods, domain, URI, name, action, middleware, excluded middleware and the file:line of the innermost app frame. Routes only package code adds are listed, not scored |
| `wiring.php` | the listeners, observers, policies, container bindings, scheduled tasks and HTTP middleware of the booted app, the gate and the schedule traced to the line that registers each entry |

Run `routes.php` and `wiring.php` from the app root with `DB_CONNECTION=sqlite DB_DATABASE=:memory:`, so
booting never touches the app's own database. A fact Laravel has and the graph lacks counts as flagged,
not missed, only when the graph has an unresolved note on the very line Laravel's registration comes
from. A fact registered by package code (a binding made from the app's `config/`, a package's own
policy) is counted as explained, with the vendor file:line or config key that proves it.

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
shot routes  'level=3&focus=routes%2Fweb.php'
shot routes-edge 'level=1&from=routes&to=App%5CHttp'
```

## Releasing

CI (`.github/workflows/ci.yml`) builds the extractor and runs `cargo test` and `pnpm test` on every
push and pull request. It has no corpora, so the laravel.io and framework invariants are skipped there.

To release, bump `version` in `extractor/Cargo.toml` and the `version` input's default in `action.yml`,
commit, and push a matching tag:

```sh
git tag v0.6.0
git push origin v0.6.0
```

`.github/workflows/release.yml` fails if the tag differs from the crate version or from the action's
default, builds the four binaries and publishes a GitHub release with the archives, `SHA256SUMS` and
generated notes. Update `VERSION` in the README's install instructions and the action's tag in its
workflow example with each release.

The action downloads the release named by its `version` input, so a tag of this repository and the
binary it uses always match. `tests/action.test.js` runs `action/report.sh` against the blog pull
request with a stand-in for `gh`.

## Dependencies

Every dependency here was at least 7 days old when it was added:

- **pnpm** enforces this with `minimumReleaseAge: 10080` in `pnpm-workspace.yaml`.
- **Cargo** has no such setting. `Cargo.lock` was generated with `--publish-time` set 7 days back, and
  the mago crates are pinned to `=1.54.0`.
- **Composer** (2.10) has no release-age setting either. Corpora without a committed lock were resolved
  with `composer update --no-install`. Any package younger than 7 days was then pinned with `--with
  vendor/package:version` before installing.
- **GitHub Actions** are pinned to the commit of a release that was at least 7 days old.
