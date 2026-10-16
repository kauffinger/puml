# Reviewer questions, answered from the PHP source

The questions an architecture reviewer asks on first contact with laravel.io, with the answers derived
from the PHP itself (grep, `php artisan route:list`, reading the files) and never from `out/laravelio.json`
or `puml-extract report`. The viewer and the terminal are scored against this file: a question counts as
answered when the reviewer names the modules, classes or numbers in the **Answer** line.

Corpus: `~/src-ref/corpora/laravelio` (PSR-4 roots `App\` → `app/`, `Database\Factories\`,
`Database\Seeders\`; `composer.json` "autoload"). Commands run from the corpus root. `route:list` needs
`DB_CONNECTION=sqlite DB_DATABASE=$PWD/database/database.sqlite`.

## laravel.io

### Q1. What are the top-level modules, and which hold the most classes?

**Answer:** one module per directory under `app/` plus `Database\Factories` and `Database\Seeders`:
26 modules. The three largest are `App\Http` (46 classes), `App\Jobs` (42) and `App\Filament` (18);
`App\Models` has only 7.

**Evidence:** `for d in app/*/; do echo "$d $(find $d -name '*.php' | wc -l)"; done` →
Actions 2, Concerns 12, Console 3, Contracts 4, Enums 1, Events 6, Exceptions 3, Filament 18, Http 46,
Jobs 42, Listeners 11, Livewire 9, Mail 4, Markdown 3, Models 7, Notifications 8, Observers 1,
Policies 5, Providers 2, Queries 3, Rules 5, Social 2, View 1; `database/factories` 7,
`database/seeders` 6; five classes sit directly in `app/` (`App\Crawler` etc.).

### Q2. Which modules are the HTTP entry layer, and which way do dependencies point between that layer and the models?

**Answer:** `App\Http`, `App\Livewire`, `App\Filament` and `App\View` are the UI. Dependencies point
from the UI to the models: every controller imports models, and no model imports anything from
`App\Http`, `App\Livewire` or `App\Filament`.

**Evidence:** `grep -rn -e 'Http\\' -e 'Livewire' -e 'Filament' app/Models` finds only the vendor
`Filament\Models\Contracts\FilamentUser` and `Filament\Panel` in `app/Models/User.php:10-11`, nothing
from `App\`. `grep -rlF 'App\Models\' app/Http | wc -l` → 22 files.

### Q3. Where does `POST /forum/create-thread` go: controller → ? → model?

**Answer:** route `threads.store` → `App\Http\Controllers\Forum\ThreadsController@store` →
`App\Jobs\CreateThread::fromRequest(ThreadRequest)` dispatched synchronously →
`CreateThread::handle()` creates `App\Models\Thread` (and a `Subscription`). The request class is
`App\Http\Requests\ThreadRequest`.

**Evidence:** `php artisan route:list` → `POST forum/create-thread threads.store ›
Forum\ThreadsController@store`; `app/Http/Controllers/Forum/ThreadsController.php:98-108`
(`store(ThreadRequest $request)`, `$this->dispatchSync(CreateThread::fromRequest($request, …))`,
`Thread::findByUuidOrFail`); `app/Jobs/CreateThread.php:23-48` (`fromRequest(ThreadRequest …)`,
`handle()` does `new Thread([...])`, `new Subscription`).

### Q4. Which dependencies break Laravel conventions?

**Answer:** seven jobs in `App\Jobs` take an `App\Http\Requests` form request in their static
`fromRequest()` constructor: CreateArticle, CreateReply, CreateThread, RegisterUser, UpdateArticle,
UpdateProfile, UpdateThread. The core (jobs) depends on the HTTP layer. Nothing else in the core, and
nothing in the UI, depends on `App\Console`; no event references a listener.

**Evidence:** `grep -rn 'App\\Http\\' app | grep -v -e '^app/Http/' -e '^app/Livewire/' -e
'^app/Filament/' -e '^app/View/' -e '^app/Providers/' -e '^app/Console/'` → exactly
`app/Jobs/CreateReply.php:8`, `UpdateThread.php:5`, `RegisterUser.php:7`, `UpdateProfile.php:6`,
`UpdateArticle.php:6`, `CreateArticle.php:6`, `CreateThread.php:6`; the type hints are at
`app/Jobs/CreateReply.php:24`, `UpdateThread.php:19`, `RegisterUser.php:21`, `UpdateProfile.php:29`,
`CreateArticle.php:30`, `UpdateArticle.php:27`, `CreateThread.php:23`.
`grep -rl 'App\\Console' app | grep -v '^app/Console/'` → nothing. `grep -rn Listeners app/Events` →
nothing.

### Q5. Which violation is the worst, and why?

**Answer:** all seven break the same convention (the core must not know the HTTP layer) and all sit on
one module edge, `App\Jobs → App\Http`. That edge is the worst because it also closes a cycle: the
controllers dispatch the jobs (`App\Http → App\Jobs`), and the jobs type-hint the requests back. One
fix, replacing the request type hints with plain values, removes every violation and breaks the cycle.

**Evidence:** Q4 for the seven edges; `grep -rl 'App\\Jobs\\' app/Http | wc -l` → 16 files in `app/Http`
import jobs, 36 `use App\Jobs\…` lines in total.

### Q6. Are there dependency cycles between modules? Which modules?

**Answer:** yes, two.
(a) The UI/jobs cycle: `App\Http ↔ App\Jobs` directly, with longer loops through `App\Actions`
(`Http → Actions → Jobs → Http`) and `App\Providers → App\Filament → Jobs → Http`. Modules: Http,
Jobs, Actions, Providers, Filament.
(b) The model cycle: `App\Models ↔ App\Policies`, `App\Models ↔ App\Exceptions`, and
`App\Models → App\Enums → App\Notifications → App\Models`, with `App\Notifications → App\Mail →
App\Models` and `App\Notifications ↔ App\Enums`. Modules: Models, Policies, Exceptions, Enums,
Notifications, Mail. `App\Concerns` and `App\Contracts` depend on Models and are used by Models too, but
they are traits and interfaces that only the models use, so they are really parts of `App\Models`.

**Evidence:** (a) Q4/Q5; `app/Actions/ConnectGitHubAccount.php:5` (`use App\Jobs\…`),
`app/Http/Controllers/Auth/GitHubController.php:5` (`use App\Actions\…`),
`app/Http/Controllers/Auth/LoginController.php:6` (`use App\Providers\AppServiceProvider`),
`app/Providers/Filament/AdminPanelProvider.php:5-10` (`use App\Filament\Widgets\…`),
`app/Filament/Resources/Users/Tables/UsersTable.php:5-10` (`use App\Jobs\…`).
(b) `app/Models/User.php:8` + `:539` (`UserPolicy::ADMIN`), `app/Policies/UserPolicy.php:5`;
`app/Models/Thread.php:19`, `app/Exceptions/CouldNotMarkReplyAsSolution.php:5`;
`app/Models/User.php:7` (`use App\Enums\NotificationType`), `app/Enums/NotificationType.php:5-6`
(`use App\Notifications\MentionNotification`, `NewReplyNotification`),
`app/Notifications/MentionNotification.php:6-7` (`use App\Enums\…`, `use App\Mail\MentionEmail`),
`app/Mail/MentionEmail.php:6` (`use App\Models\User`). Concerns/Contracts:
`grep -rl 'App\\Models\\' app/Concerns | wc -l` → 6, `grep -rl 'App\\Concerns\\' app/Models | wc -l` → 6.

### Q7. What is the cheapest cut of the Http/Jobs cycle?

**Answer:** remove the seven `App\Jobs → App\Http` edges, i.e. the seven `fromRequest()` type hints of
Q4 (7 class edges in 7 files). Every loop of that cycle passes through `Jobs → Http`; any cut that
keeps it must remove `Http → Jobs`, which is 36 import lines in 16 files.

**Evidence:** Q4, Q5, Q6(a). The three loops are `Http → Jobs → Http`, `Http → Actions → Jobs → Http`
and `Http → Providers → Filament → Jobs → Http`; `Jobs → Http` is on all three.

### Q8. Which module is the riskiest to change (most depended upon)?

**Answer:** `App\Models`. 133 classes outside it (in 136 files, three of which only name a model in a
`@mixin` docblock) use its seven models; it depends on only 17 classes of other app modules. Next:
`App\Jobs` (23 files), `App\Events` (17), `App\Contracts` and `App\Policies` (15 each).

**Evidence:** `grep -rlF 'App\Models\' app database | grep -v '^app/Models/' | wc -l` → 136;
`app/Http/Resources/{Article,Author,Tag}Resource.php` only mention the model in `@mixin`. Per-module
counts: `for d in app/*/; do m=$(basename $d); grep -rlF "App\\$m\\" app database | grep -v "^app/$m/" | wc -l; done`.

### Q9. Which module depends on the most other modules' classes, and which class is the busiest?

**Answer:** `App\Http` uses 56 distinct classes of other app modules (Jobs 23, Models 17, Livewire 16,
Listeners 15 follow). The busiest class is `App\Http\Controllers\Forum\ThreadsController`, importing 17
app classes outside `App\Http`; `App\Models\Thread` is second with 15.

**Evidence:** `grep -rhoE 'use App\\[A-Za-z]+\\[A-Za-z\\]+' app/Http | sort -u | grep -v 'App\\Http\\' | wc -l`
→ 56; `app/Http/Controllers/Forum/ThreadsController.php:5-23` (17 `use App\…` lines outside Http);
`grep -c '^use App' app/Models/Thread.php` → 15.

### Q10. Where should a newcomer start reading?

**Answer:** `App\Models`, because everything depends on it and it depends on little (Q8); within it
`App\Models\Thread`, the most connected model (Q9). Then follow one request from
`App\Http\Controllers\Forum\ThreadsController` through `App\Jobs\CreateThread` (Q3). Either
`App\Models`/`Thread` or `ThreadsController` is accepted.

### Q11. Do events know their listeners?

**Answer:** no. Dependencies point from `App\Listeners` to `App\Events` (10 listener files import an
event); no event imports a listener.

**Evidence:** `grep -rl 'App\\Events\\' app/Listeners | wc -l` → 10; `grep -rl 'App\\Listeners' app/Events`
→ nothing.

### Q12. Which modules does nothing else in the application depend on (the leaves)?

**Answer:** `App\Console`, `App\Listeners`, `App\Livewire`, `App\Markdown`, `App\Queries`, `App\View`,
`Database\Factories` and `Database\Seeders`.

**Evidence:** the per-module grep of Q8 reports 0 referencing files for exactly these eight.

## Seeded blog copy (`/tmp/puml-blog-seeded`)

`fixtures/blog` was copied to `/tmp/puml-blog-clean` with every defect it ships with removed (verified:
zero violations, zero cycles), then copied to `/tmp/puml-blog-seeded` with three defects planted:

### S1. A model depends on a controller

`App\Models\User::profileUrl()` calls `App\Http\Controllers\ProfileController::urlFor()`
(`app/Models/User.php:5`, `:17`). Rule broken: the core must not depend on the HTTP layer.

### S2. A new cycle between two modules

`App\Notifications\PostPublishedNotification::toMail()` builds `App\Mail\PostPublishedMail`
(`app/Notifications/PostPublishedNotification.php:9,11`), and the mail takes the notification in its
constructor (`app/Mail/PostPublishedMail.php:5,9`). Cycle: `App\Mail ↔ App\Notifications`. No rule is
broken by it.

### S3. A job takes a form request

`App\Jobs\DeletePost::fromRequest(DeletePostRequest $request)` (`app/Jobs/DeletePost.php:5`, `:14`).
Rule broken: the core must not depend on the HTTP layer.

S1 and S3 also close a cycle `App\Http → App\Jobs → App\Models → App\Http` with the existing
`PostController → PublishPost → Post` edges; that cycle is a consequence, not a seeded defect.

The judge's questions for the copy: which rules are broken and by which classes (expects S1 and S3,
and nothing else), which module cycles exist (expects Mail ↔ Notifications, and accepts the
Http/Jobs/Models one), and what is the cheapest cut of each.
