# Reviewer measurements, laravel.io and the seeded blog

How fast a reviewer answers the questions of `reviewer-questions.md` with the viewer, before and after the
viewer changes on the `puml-viewer` branch. Everything here is rerunnable:

```sh
python3 -m http.server 8765 --bind 127.0.0.1
node verify/drive.mjs out/shots/after verify/tasks-laravelio.json
node verify/drive.mjs out/shots/after-blog verify/tasks-blog-seeded.json
node verify/drive.mjs out/shots/before-2 verify/tasks-laravelio-before.json   # against out/before/
```

`drive.mjs` replays each task's shortest path in headless Chrome at 1280×800. A task is **answered** when
every expected string has been shown, and **readable** when it was shown on screen at ≥ 11 px without
scrolling. Answers that are still below the fold after the last step are found by paging the side panel
down, and each page counts as one interaction. "Clicks + pages" below is that count. `measure.js` records the
smallest rendered label (font size × SVG scale), the nodes and edges drawn, and the render time from the
stats line.

## Per question (laravel.io)

| Q | Question | Before: clicks + pages | After: clicks + pages | Terminal | Before judge | After judge |
|---|---|---|---|---|---|---|
| Q1 | Modules, largest | 0 + 3, labels 5.9 px (unreadable) | 0 + 3 | 1 `report` | correct | correct: 26 modules, Http 46, Jobs 42, Filament 18 (0) |
| Q2 | Entry layer, direction | 5 + 0 | 1 + 0 | 1 `report` | correct | correct: Http/Filament/Livewire/View, Http → Models 38, nothing back (1) |
| Q3 | `threads.store` path | 2 + 0 | 2 + 0 | 2 `show` | correct | correct: ThreadsController::store → CreateThread → Thread, Subscription (3) |
| Q4 | Convention breaks | 0 + 0 | 0 + 0 | 1 `report` | correct | correct: all seven jobs with file:line (0) |
| Q5 | Worst violation | 0 + 2 | 0 + 1 | 1 `report` | correct | correct: Jobs → Http closes the Http/Jobs cycle (0) |
| Q6 | Module cycles | 0 + 2 | 0 + 1 | 1 `report` | correct | correct: both cycles with members (0) |
| Q7 | Cheapest cut | 0 + 2 | 0 + 1 | 1 `report` | correct | correct: 7 class edges in 7 files (0) |
| Q8 | Riskiest module | 26 + 0 | 0 + 2 | 1 `report` | correct | correct: App\Models, Ca 133 (0) |
| Q9 | Most outgoing, busiest class | 28 + 0 | 0 + 3 | 1 `report` | partly: busiest class wrong (User, by fan-in) | correct: App\Http Ce 56, ThreadsController fan-out 17 (0) |
| Q10 | Where to start | 26 + 0 | 0 + 2 | 1 `report` | correct | correct: App\Models, then ThreadsController → CreateThread (0) |
| Q11 | Events know listeners? | 1 + 0 | 1 + 0 | 1 `report` or `show App\Events` | correct | correct: no, Listeners → Events (1) |
| Q12 | Leaves | 26 + 0 | 0 + 2 | 1 `report` | correct (after 15 clicks) | correct: all eight (0) |

Before the change, Q8, Q9, Q10 and Q12 meant clicking every module and reading Ca/Ce off each panel, since
the viewer had no ranking (26 and 28 clicks). The before judge answered them with fewer clicks only by
reading every panel's text with the free `panel` command.

Q2's expectation was changed once during the work. It had asked for the rule text `core ↛ http`, which
states the rule, not the fact. It now asks for what the Models panel shows: its "Depends on" list, which holds
core modules only, and "Used by … App\Http 38". The before task asked for even less (`layer http`,
`Used by`).

Screenshots: `out/shots/before-2/<task>.png` and `out/shots/after/<task>.png`, for example
`out/shots/after/q04-violations.png` (opening view) and `out/shots/after/q03-create-thread.png`.

## Seeded blog

| Defect | Before: clicks + pages | After: clicks + pages | On the opening view |
|---|---|---|---|
| S1 `User → ProfileController` (model → controller) | 0 + 0 | 0 + 0 | yes: purple edge, listed under Rules |
| S2 `Mail ↔ Notifications` cycle | 0 + 1 | 0 + 1 | the nodes are outlined red and the cycle is drawn; its members and cut are one page down |
| S3 `DeletePost → DeletePostRequest` (job → form request) | 0 + 0 | 0 + 0 | yes: purple edge, listed under Rules |

The opening view went from 11 nodes, 20 edges and 11.6 px to 10 nodes, 15 edges and 16.1 px.
Screenshots: `out/shots/before-2-blog/` and `out/shots/after-blog/`.

## Per view (CLAUDE.md checklist)

| View | Before: nodes / edges, min px, ms | After: nodes / edges, min px, ms |
|---|---|---|
| laravel.io level 1 | 40 / 60, 5.9 px, 75 ms | 26 / 30, 11 px, 37 ms |
| laravel.io `App\Http` | 23 / 33, 6.6 px, 36 ms | 23 / 33, 12.8 px, 45 ms |
| laravel.io `App\Http\Controllers` | 33 / 60, 7.5 px, 51 ms | 33 / 60, 11 px, 52 ms, cropped |
| laravel.io edge Controllers → Models | 22 / 27, 7.2 px, 28 ms | 22 / 27, 11 px, 28 ms, cropped |
| framework level 1 | 40 / 60, 5.9 px, 85 ms | 39 / 36, 11 px, 49 ms, cropped |
| framework `Illuminate\Database\Eloquent` | 40 / 60, 4.1 px, 74 ms | 40 / 60, 11 px, 72 ms, cropped |
| framework edge Relations → Model (`neato`) | 21 / 20, 9.7 px, 36 ms | 21 / 20, 13.3 px, 38 ms |

"Cropped" means the drawing would need a scale below 1 to fit. The viewer then keeps labels at 11 px,
shows the part around the focus, and says "part off-screen, show all" in the stats line. Panning or
"show all" reveals the rest.

`puml.benchmark` on the framework (median / max, ms), after: level 1 21.2 / 23.5; level 1 with vendor
20.9 / 22.6; `Illuminate\Database` 44.0 / 54.5; `Eloquent` 39.8 / 43.6; Relations → Model 11.5 / 13.2;
`Illuminate\Support` 28.3 / 28.8; UML `Model` 18.7 / 20.6. Before, level 1 had a median of 51.7 and the maximum was 57.3.

Select dims the rest, double-click drills, and Esc goes up and reselects the node you came from. All three pass
(`check-select-dims`, `check-drill-and-up`).

## Terminal

`puml-extract report` takes 0.03 s and prints 121 lines. The sections start at these lines: RULES 4,
MODULE CYCLES 23, MODULES 68, FAN-OUT OUTLIERS 100 and LAYERS 111. `show` takes 0.03 s:
ThreadsController prints 66 lines and CreateThread 37. Q3 takes two `show` commands, controller and then
job. Every other question takes one `report`, though in a terminal 800 px high the reader has to scroll it as well.

## What changed in the viewer, by the tasks it sped up

1. Level 1 shows the layers as bands (http, core, console, providers, database). Upward edges don't
   constrain the ranks, and the spacing is tight. Level 1 draws the 30 heaviest module edges, and
   violations always come first. This took level 1 from 5.9 px to ≥ 11 px with no cropping, and shows the
   layers and the direction of dependencies on the opening view. It helps Q1, Q2, Q4–Q7 and S1–S3.
2. The overview panel ranks the modules. It shows a headline with the counts, the violations with
   file:line, the layers and the forbidden directions, the cycles with their cut and its evidence, the
   five busiest classes by fan-out, a table of modules sorted by Ca with Ce, and the leaves (Ca 0). This
   took Q8, Q9, Q10 and Q12 from 26–28 clicks to 2–3 page-downs.
3. Every view has a zoom floor of scale 1. Labels never shrink below 11 px, and a cropped view says so and
   offers "show all". This fixed every checklist view below 11 px. A dot layout that would need a scale
   below 1 gives way to `neato` when `neato` fits better; before, dot was kept down to 0.8, which the floor
   then cropped (`App\Http` at 1600×1000).
4. `visibleEdges` ranks the edges that break a rule first. In the banded level 1, the heaviest edges
   come before cycle edges. The judge had found `Http → Models` (38) and `Jobs → Models` (118) hidden
   behind light cycle edges.

## Where the viewer is still worse than the terminal

- **Q1, Q8, Q9, Q10, Q12**: 2–3 page-downs against one `report`. The answers sit in the module table and
  the busiest-class list. These come after the violations, and the violations stay first so that Q4 needs
  0 interactions. Q1 can also be read off the drawing at 0 (each node shows its class count, at ≥ 11 px).
  The driver only counts it when the table row is on screen, though.
- **Q3**: equal at 2. The controller → job → model path needs a search and a double-click, as the terminal
  needs two `show` commands.
- **Cropped views**: four of the seven checklist views fit only at a scale below 1. They are legible, but
  the reviewer has to pan or use "show all", which the terminal never needs.

## Judge

A fresh agent with only the served HTML (`verify/judge-prompt.md`, driven through `verify/browse.mjs`)
answered the questions, then scored itself against this file's sibling `reviewer-questions.md`.

- Before (original viewer): 14 correct, 1 partly correct (Q9: it named `App\Models\User` as the busiest
  class, by fan-in, because the viewer had no fan-out ranking), 0 wrong. It found all three seeded defects.
- After (final build): 15 of 15 correct, 0 partly, 0 wrong, all three seeded defects found, 5 counted
  interactions in total (Q2 1, Q3 3, Q11 1). The judge's counts in brackets in the table above are its own.
  It read the panel below the fold with the free `panel` command, so they are lower than the driver's
  clicks + pages. What still slowed it down: answers below the fold of the panel, a UML view taller
  than the viewport (ThreadsController hides CreateThread until panned or "show all"), no severity
  ranking or "start here" hint (Q5, Q10 were its own judgement), and module edge counts that sit close
  together near `App\Models`.
