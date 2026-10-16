<img src="assets/icon.svg" width="64" height="64" alt="">

# puml

A module and UML viewer for PHP and Laravel codebases that works at three levels of detail. A Rust
extractor, `puml-extract`, uses [mago](https://github.com/carthage-software/mago)'s crates to turn a
project into a versioned JSON graph. A static viewer then draws that graph with Graphviz (WASM) in the
browser.

```
extractor/   puml-extract (Rust, links mago 1.54.0)
viewer/      index.html, app.js (rendering), model.js (levels, aggregation, cycles, budgets)
fixtures/    shop: a small fixture app with a hand-written expected graph
tests/       golden, invariant, drill, cycle and edge-budget tests on the JSON contract (node --test)
verify/      PHP judges that compare a graph with what PHP itself reports
```

## Usage

```sh
cargo build --release --manifest-path extractor/Cargo.toml
extractor/target/release/puml-extract path/to/laravel-app --out out/app.json --html out/app.html
open out/app.html
```

`--html` writes one self-contained file with the viewer and the graph embedded in it. `--dev` also
scans the `autoload-dev` roots. While working on the viewer, serve the repository and open
`viewer/index.html?graph=../out/app.json`. The viewer is compiled into the binary, so rebuild after you
change it.

The view state lives in the URL hash, for example `#level=2&focus=App%5CModels&budget=30&kinds=extends,relation`.

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

## Levels of detail

**Level 1, modules.** A node is a module:

- a PSR-4 root from `composer.json` plus the next namespace segment, such as `App\Models` or
  `Illuminate\Database`
- or a whole vendor package, such as `laravel/framework`, collapsed into one node
- or `php` for built-in classes

Every module that declares a class is shown. A vendor package is shown only when an edge reaches it,
and the `vendor` checkbox hides packages altogether.

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
constants, properties and methods, with visibility (`+ # -`), types as written (including PHPDoc
generics such as `list<Product>`). Its Eloquent relations are drawn as `relation` edges and listed in
the side panel. Every class sharing an edge with the focus is drawn as a plain node.

**Aggregation.** At every level, an edge between two nodes aggregates every class edge between their
classes. Its weight is the number of class edges under it. Hovering over it lists the class pairs
behind it, heaviest first, and clicking it lists them in the panel with their `file:line` evidence.
Levels 2 and 3 only show edges that touch the focus.

**Edge view.** Double-clicking an aggregated edge opens it: the classes at both ends, grouped in one
box per end, and only the class edges between them. The URL is the base view plus `from` and `to`, for
example `#level=2&focus=App%5CHttp&from=App%5CHttp%5CControllers&to=App%5CModels`.

**Cycles.** Cycles are the strongly connected components of the projected graph, found with Tarjan's
algorithm. They are drawn in red and listed in the side panel.

**Node budget.** The budget is 40 nodes at levels 1 and 2, and 24 at level 3, where the focused class
alone takes the room of many plain nodes. Past the budget, the lowest-ranked nodes merge into one
"N more" node, which you can click to list them. Cycles are found before the merge, so merging cannot
create one.

**Edge budget.** At most 60 edges are drawn:

1. the heaviest edge of each node, so no node loses all of its edges
2. then cycle edges before others, heaviest first, until the budget is reached

Hidden edges are listed in the panel.

**Layout.** The viewer tries three layouts and keeps the most legible one:

- First Graphviz `dot`, top to bottom (bottom to top at level 3).
- If that does not fit, `dot` left to right.
- If neither reaches a scale of 0.8, `neato`, without the contents lines and the edge view's end boxes,
  which `neato` cannot place. Whichever layout has the larger scale wins.

## Navigating

| action | effect |
|---|---|
| click a node or edge | selects it, dims everything not attached to it, and shows it in the panel |
| double-click a node | opens it: a module, package or namespace becomes the focus, a class opens as its UML |
| double-click an edge | opens the edge view |
| Esc, Backspace or `↑ up` | goes to the parent view and selects the node you came from |
| breadcrumb | jumps to any ancestor namespace |

The panel lists a node's contents, what it depends on and what uses it, each as a link. Every
`file:line` can open in VS Code, Cursor or PhpStorm. Pick the editor in the toolbar; the choice is
kept in `localStorage`. The links are built from the graph's `path`, the absolute project root at
extraction time, so they only work on the machine that ran the extractor.

## Edge kinds

Every edge comes from a name that mago resolved; none come from regular expressions.

| kind | source |
|---|---|
| `extends`, `implements`, `uses-trait` | class declarations |
| `injects` | constructor parameter types, including promoted properties |
| `property` | property types, native and `@var` |
| `signature` | parameter and return types of other methods, native and PHPDoc, including generic arguments |
| `relation` | `$this->hasMany(Post::class)` and the other Eloquent relation methods, read from the method body; labelled `posts(): HasMany` |
| `new` | `new Foo` |
| `static` | `Foo::bar()`, `Foo::CONST`, `Foo::$prop` |
| `facade` | static calls on a facade, drawn to the facade's underlying class |
| `reference` | `Foo::class`, `instanceof Foo`, `catch (Foo $e)` |

Facades are resolved from `getFacadeAccessor()`:

- If it returns `Foo::class`, the facade resolves to that class.
- If it returns a string key, the key is looked up in `Application::registerCoreContainerAliases()`.
- Real-time facades (`Facades\App\Foo`) resolve to `App\Foo`.
- Global aliases such as `\Route` are resolved through `Facade::defaultAliases()`.

Anything that cannot be resolved statically goes into the graph's `unresolved` list with its
`file:line`. Such cases are never guessed.

## Graph JSON (version 1)

```
{ version, project, path, generator, roots, packages, classes, edges, unresolved, stats }
edge: { from, to, kind, label?, evidence: [{ file, line }] }
```

The JSON is the contract under test. `fixtures/shop/expected/` holds the hand-written expected graph,
and the expected projection at each level.

## Verification

```sh
pnpm test
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

## Dependencies

Every dependency here was at least 7 days old when it was added:

- **pnpm** enforces this with `minimumReleaseAge: 10080` in `pnpm-workspace.yaml`.
- **Cargo** has no such setting. `Cargo.lock` was generated with `--publish-time` set 7 days back, and
  the mago crates are pinned to `=1.54.0`.
- **Composer** (2.10) has no release-age setting either. Corpora without a committed lock were resolved
  with `composer update --no-install`. Any package younger than 7 days was then pinned with `--with
  vendor/package:version` before installing.
