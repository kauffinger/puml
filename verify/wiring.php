<?php

/*
 * Compares what a puml graph says Laravel wires up at boot with what the booted app holds.
 *
 * usage: php verify/wiring.php <graph.json> <laravel-app-root> [--json report.json]
 *
 * The app is booted through its console kernel, and Artisan is started so `withSchedule()` callbacks run.
 * Every app model is then instantiated once, which boots it and registers its #[ObservedBy] observers.
 * Each section counts what the graph and Laravel agree on, what only the graph says (fp) and what only
 * Laravel holds (fn). The gate and the schedule are swapped for subclasses that record the call stack of
 * every policy and task; one is the app's when a frame lies in an app file or runs on an app object, as a
 * service provider's `$policies` do. Elsewhere only wiring that involves the app's own classes is scored,
 * on both sides:
 *
 * - listeners: Dispatcher::getRawListeners() whose event or listener is a class of the app, or whose
 *   closure is written in one of its files. Model events (`eloquent.*`) are the observers'.
 * - observers: the `eloquent.{event}: {Model}` listeners an app observer class registers, as model,
 *   observer and the set of events.
 * - policies: Gate::getPolicyFor() for every class of the app, and the policies app code registers.
 * - bindings: Container::getBindings() whose abstract or concrete is a class of the app, or whose closure
 *   is written in one of its files. A closure's concrete is the class it makes, if it can be made here.
 * - schedule: the tasks app code adds, as task and target, the class Artisan runs for a command, and the
 *   file:line that adds them.
 * - middleware: the HTTP kernel's global middleware, groups and aliases, for middleware classes of the app.
 *
 * A fact the graph lists as unresolved on the line Laravel's registration comes from is counted as flagged,
 * not missed, and so is a binding whose concrete the graph leaves open with a note on that line. A class
 * PHP cannot load (it implements a missing interface, or no longer matches its parent) is left out and
 * listed, since loading it would end the process.
 */

use Illuminate\Auth\Access\Gate as AccessGate;
use Illuminate\Console\Scheduling\CallbackEvent;
use Illuminate\Console\Scheduling\Schedule;
use Illuminate\Contracts\Auth\Access\Gate as GateContract;
use Illuminate\Contracts\Console\Kernel;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Support\Facades\Artisan;
use Illuminate\Support\Facades\Facade;
use Illuminate\Support\Facades\Gate;

ini_set('display_errors', 'stderr');

[$graphPath, $root] = [$argv[1] ?? null, $argv[2] ?? null];

if (! $graphPath || ! $root) {
    fwrite(STDERR, "usage: php verify/wiring.php <graph.json> <laravel-app-root> [--json report.json]\n");
    exit(2);
}

$reportPath = ($index = array_search('--json', $argv, true)) !== false ? $argv[$index + 1] : null;
$graph = json_decode(file_get_contents($graphPath), true, flags: JSON_THROW_ON_ERROR);
$root = realpath($root);

$appClasses = [];

foreach ($graph['classes'] as $class) {
    if (! $class['external'] && $class['kind'] !== 'file') {
        $appClasses[strtolower($class['id'])] = $class['id'];
    }
}

/*
 * Loads every class in child processes, one after another, and returns those that load. A class that ends
 * its process with a fatal error is the last one the process named.
 */
function loadable(array $classes, string $root): array
{
    $pending = array_values($classes);
    $loaded = [];
    $broken = [];
    $list = tempnam(sys_get_temp_dir(), 'puml-classes');

    while ($pending !== []) {
        file_put_contents($list, json_encode($pending));
        $code = 'require $argv[1]; foreach (json_decode(file_get_contents($argv[2]), true) as $class) { echo $class, "\n"; try { class_exists($class) || interface_exists($class) || trait_exists($class) || enum_exists($class); echo "ok\n"; } catch (Throwable $e) { echo "error\n"; } }';
        $output = [];
        exec(sprintf('%s -d display_errors=stderr -r %s %s %s 2>/dev/null', escapeshellarg(PHP_BINARY), escapeshellarg($code), escapeshellarg("$root/vendor/autoload.php"), escapeshellarg($list)), $output);

        $last = null;
        $checked = 0;

        foreach ($output as $line) {
            if ($line === 'ok' || $line === 'error') {
                if ($line === 'ok') {
                    $loaded[] = $last;
                } else {
                    $broken[] = $last;
                }

                $last = null;
                $checked++;
            } else {
                $last = $line;
            }
        }

        if ($checked < count($pending)) {
            $broken[] = $pending[$checked];
            $checked++;
        }

        $pending = array_slice($pending, $checked);
    }

    unlink($list);

    return [$loaded, $broken];
}

[$loadable, $broken] = loadable($appClasses, $root);
$loadable = array_flip(array_map('strtolower', $loadable));

require "$root/vendor/autoload.php";

final class TracingGate extends AccessGate
{
    public static array $stacks = [];

    public function policy($class, $policy)
    {
        self::$stacks[ltrim($class, '\\')] = debug_backtrace(DEBUG_BACKTRACE_PROVIDE_OBJECT);

        return parent::policy($class, $policy);
    }
}

final class TracingSchedule extends Schedule
{
    public static array $stacks = [];

    public static array $jobs = [];

    public function job($job, $queue = null, $connection = null)
    {
        $event = parent::job($job, $queue, $connection);
        self::$jobs[spl_object_id($event)] = is_string($job) ? ltrim($job, '\\') : get_class($job);

        return $event;
    }

    public function exec($command, array $parameters = [])
    {
        return $this->traced(parent::exec($command, $parameters));
    }

    public function call($callback, array $parameters = [])
    {
        return $this->traced(parent::call($callback, $parameters));
    }

    private function traced($event)
    {
        self::$stacks[spl_object_id($event)] ??= debug_backtrace(DEBUG_BACKTRACE_PROVIDE_OBJECT);

        return $event;
    }
}

$app = require "$root/bootstrap/app.php";

if (file_exists($app->getCachedEventsPath())) {
    fwrite(STDERR, "the events of $root are cached; run `php artisan event:clear` first\n");
    exit(2);
}

$kernel = $app->make(Kernel::class);

$app->booting(function () use ($app) {
    $previous = $app->resolved(GateContract::class) ? $app->make(GateContract::class) : null;
    $state = $previous ? (fn () => [$this->abilities, $this->policies, $this->beforeCallbacks, $this->afterCallbacks])->call($previous) : [[], [], [], []];

    $app->singleton(GateContract::class, fn ($app) => new TracingGate($app, fn () => call_user_func($app['auth']->userResolver()), ...$state));
    Facade::clearResolvedInstance(GateContract::class);
});

$app->booted(function () use ($app, $kernel) {
    $app->singleton(Schedule::class, fn () => (fn () => tap(new TracingSchedule($this->scheduleTimezone()), fn ($schedule) => $this->schedule($schedule->useCache($this->scheduleCache()))))->call($kernel));
    Facade::clearResolvedInstance(Schedule::class);
});

$kernel->bootstrap();
$commands = Artisan::all();

$isAppClass = fn (?string $class) => $class !== null && isset($appClasses[strtolower(ltrim($class, '\\'))]);
$isLoadable = fn (string $class) => isset($loadable[strtolower($class)]);
$relative = fn (string $file) => str_starts_with($file, "$root/") ? substr($file, strlen($root) + 1) : $file;
$isAppFile = fn (string $file) => str_starts_with($file, "$root/") && ! str_starts_with($file, "$root/vendor/");
$canonical = fn (string $class) => $appClasses[strtolower(ltrim($class, '\\'))] ?? ltrim($class, '\\');

$failures = array_map(fn ($class) => "$class cannot be loaded by PHP and is not checked", $broken);

/*
 * Who registered a policy or a task: the first frame outside the gate, the schedule, facades and the
 * container. It is the app's when that call is written in an app file, or runs on an app object, as the
 * loop over a provider's `$policies` does in the framework's AuthServiceProvider.
 */
$appFrame = function (array $stack) use ($isAppFile, $isAppClass, $relative, $root): ?array {
    $plumbing = array_map(fn ($directory) => "$root/vendor/laravel/framework/src/Illuminate/$directory/", ['Auth/Access', 'Console/Scheduling', 'Support/Facades', 'Container', 'Foundation/Console']);

    foreach ($stack as $index => $frame) {
        if (! isset($frame['file']) || array_filter($plumbing, fn ($directory) => str_starts_with($frame['file'], $directory)) || realpath($frame['file']) === realpath(__FILE__)) {
            continue;
        }

        if ($isAppFile($frame['file'])) {
            return ['at' => $relative($frame['file']).':'.$frame['line']];
        }

        $caller = $stack[$index + 1]['object'] ?? null;

        return $caller !== null && $isAppClass(get_class($caller)) ? ['at' => null] : null;
    }

    return null;
};

foreach ($appClasses as $class) {
    if ($isLoadable($class) && class_exists($class) && is_subclass_of($class, Model::class) && ! (new ReflectionClass($class))->isAbstract()) {
        try {
            new $class;
        } catch (Throwable $exception) {
            $failures[] = "new $class: {$exception->getMessage()}";
        }
    }
}

$flaggedLines = [];

foreach ($graph['unresolved'] as $note) {
    $flaggedLines[$note['file'].':'.$note['line']] = $note['detail'];
}

$sections = [];

$score = function (string $section, array $expected, array $actual, array $fields = [], array $explained = [], ?Closure $noted = null) use (&$sections, $flaggedLines) {
    $result = ['tp' => 0, 'fp' => 0, 'fn' => 0, 'flagged' => 0, 'explained' => 0, 'fields' => array_fill_keys($fields, 0), 'mismatches' => [], 'flagged_facts' => [], 'explained_facts' => []];

    foreach ($explained as $key => $reason) {
        if (array_key_exists($key, $expected) || array_key_exists($key, $actual)) {
            $result['explained']++;
            $result['explained_facts'][] = "$key: $reason";
            unset($expected[$key], $actual[$key]);
        }
    }

    foreach ($expected as $key => $fact) {
        if (! array_key_exists($key, $actual)) {
            $result['fp']++;
            $result['mismatches'][] = "$key: only in the graph";

            continue;
        }

        $result['tp']++;

        foreach ($fields as $field) {
            if ($fact[$field] === $actual[$key][$field]) {
                $result['fields'][$field]++;
            } elseif ($fact[$field] === null && isset($fact['at'], $flaggedLines[$fact['at']])) {
                $result['flagged']++;
                $result['flagged_facts'][] = "$key: $field at {$fact['at']}: {$flaggedLines[$fact['at']]}";
            } elseif ($noted && ($note = $noted($key))) {
                $result['flagged']++;
                $result['flagged_facts'][] = "$key: $field differs, and the graph notes {$note}";
            } elseif (is_string($actual[$key][$field]) && str_starts_with($actual[$key][$field], 'cannot be made')) {
                $result['explained']++;
                $result['explained_facts'][] = "$key: $field: the graph says ".json_encode($fact[$field], JSON_UNESCAPED_SLASHES).", which cannot be checked here: Laravel {$actual[$key][$field]}";
            } else {
                $result['mismatches'][] = "$key: $field: graph ".json_encode($fact[$field], JSON_UNESCAPED_SLASHES).', Laravel '.json_encode($actual[$key][$field], JSON_UNESCAPED_SLASHES);
            }
        }
    }

    foreach (array_diff_key($actual, $expected) as $key => $fact) {
        if (isset($fact['at']) && isset($flaggedLines[$fact['at']])) {
            $result['flagged']++;
            $result['flagged_facts'][] = "$key at {$fact['at']}: {$flaggedLines[$fact['at']]}";

            continue;
        }

        if ($noted && ($note = $noted($key))) {
            $result['flagged']++;
            $result['flagged_facts'][] = "$key: the graph notes {$note}";

            continue;
        }

        $result['fn']++;
        $result['mismatches'][] = "$key: only in Laravel".(isset($fact['at']) ? " ({$fact['at']})" : '');
    }

    $sections[$section] = $result;
};

// Listeners.

$listenerName = function ($listener) use ($relative): ?array {
    if ($listener instanceof Closure) {
        $function = new ReflectionFunction($listener);

        return ['closure', null, $relative($function->getFileName()).':'.$function->getStartLine()];
    }

    if (is_array($listener) && is_string($listener[0] ?? null)) {
        return [ltrim($listener[0], '\\').'@'.$listener[1], ltrim($listener[0], '\\'), null];
    }

    if (is_string($listener)) {
        [$class, $method] = str_contains($listener, '@') ? explode('@', $listener, 2) : [$listener, 'handle'];

        if (! str_contains($listener, '@') && ! method_exists($class, 'handle') && method_exists($class, '__invoke')) {
            $method = '__invoke';
        }

        return [ltrim($class, '\\').'@'.$method, ltrim($class, '\\'), null];
    }

    return null;
};

$actual = [];

$modelOrViewEvent = fn (string $event) => str_starts_with($event, 'eloquent.') || str_starts_with($event, 'composing: ') || str_starts_with($event, 'creating: ');

foreach ($app['events']->getRawListeners() as $event => $listeners) {
    if ($modelOrViewEvent($event)) {
        continue;
    }

    foreach ($listeners as $listener) {
        [$name, $class, $at] = $listenerName($listener) ?? [null, null, null];

        if ($name === 'closure' && $isAppFile($root.'/'.explode(':', $at)[0])) {
            $actual["$event | closure in ".explode(':', $at)[0]] = ['at' => $at];
        } elseif ($name !== null && $name !== 'closure' && ($isAppClass($class) || $isAppClass($event))) {
            $actual[$canonical($event).' | '.$canonical($class).'@'.explode('@', $name)[1]] = [];
        }
    }
}

$expected = [];

foreach ($graph['laravel']['listeners'] as $listener) {
    if ($modelOrViewEvent($listener['event'])) {
        continue;
    }

    if ($listener['listener'] === 'closure') {
        $expected["{$listener['event']} | closure in {$listener['file']}"] = ['at' => "{$listener['file']}:{$listener['line']}"];
    } elseif ($isAppClass($listener['class'] ?? null) || $isAppClass($listener['event'])) {
        $expected["{$listener['event']} | {$listener['listener']}"] = ['at' => "{$listener['file']}:{$listener['line']}"];
    }
}

$score('listeners', $expected, $actual);

// Observers.

$actual = [];

foreach ($app['events']->getRawListeners() as $event => $listeners) {
    if (! preg_match('/^eloquent\.(\w+): (.+)$/', $event, $match)) {
        continue;
    }

    foreach ($listeners as $listener) {
        if (! is_string($listener) || ! str_contains($listener, '@')) {
            continue;
        }

        [$observer, $method] = explode('@', $listener, 2);

        if ($isAppClass($observer)) {
            $actual[$canonical($match[2]).' observed by '.$canonical($observer)]['events'][] = $match[1];
        }
    }
}

foreach ($actual as &$fact) {
    sort($fact['events']);
}

unset($fact);

$expected = [];

foreach ($graph['laravel']['observers'] as $observer) {
    $events = $observer['events'];
    sort($events);
    $key = "{$observer['model']} observed by {$observer['observer']}";
    $expected[$key] = ['events' => [...($expected[$key]['events'] ?? []), ...$events]];
    sort($expected[$key]['events']);
}

/*
 * Observers are registered as plain event listeners, with no stack to tell where. A missing one counts as
 * flagged when the graph notes an observer registration of that model and observer it cannot be sure of.
 */
$observerNote = function (string $key) use ($graph): ?string {
    [$model, $observer] = explode(' observed by ', $key);

    foreach ($graph['unresolved'] as $note) {
        if ($note['kind'] === 'observer' && str_contains($note['detail'], $model) && str_contains($note['detail'], $observer)) {
            return "{$note['file']}:{$note['line']}: {$note['detail']}";
        }
    }

    return null;
};

$score('observers', $expected, $actual, ['events'], [], $observerNote);

// Policies.

$actual = [];
$explained = [];

$packagePolicies = [];

foreach (Gate::policies() as $model => $policy) {
    $model = ltrim($model, '\\');
    $stack = TracingGate::$stacks[$model] ?? null;

    if ($stack !== null && $appFrame($stack) === null) {
        $explained["$model → ".$canonical($policy)] = 'registered by package code, which the graph does not read';
        $packagePolicies[ltrim($policy, '\\')] = $model;
    } elseif (! $isAppClass($model)) {
        $actual["$model → ".$canonical($policy)] = [];
    }
}

foreach ($appClasses as $class) {
    if (! $isLoadable($class) || (! class_exists($class) && ! interface_exists($class) && ! enum_exists($class))) {
        continue;
    }

    try {
        $policy = Gate::getPolicyFor($class);
    } catch (Throwable $exception) {
        $failures[] = "Gate::getPolicyFor($class): {$exception->getMessage()}";

        continue;
    }

    if ($policy === null) {
        continue;
    }

    $policyClass = get_class($policy);

    if (isset($packagePolicies[$policyClass]) && is_a($class, $packagePolicies[$policyClass], true)) {
        $explained["$class → ".$canonical($policyClass)] = "the policy package code registers for {$packagePolicies[$policyClass]}, which $class extends or implements";
    } else {
        $actual["$class → ".$canonical($policyClass)] = [];
    }
}

$expected = [];

foreach ($graph['laravel']['policies'] as $policy) {
    $expected["{$policy['model']} → {$policy['policy']}"] = [];
}

$score('policies', $expected, $actual, [], $explained);

// Bindings.

$scoped = (fn () => $this->scopedInstances)->call($app);
$actual = [];
$explained = [];
$bound = [];

foreach ($graph['laravel']['bindings'] as $binding) {
    $bound[$binding['key']] = $binding;
}

$packageOf = function (string $file) use ($relative): ?string {
    return preg_match('#^vendor/([^/]+/[^/]+)/#', $relative($file), $match) ? $match[1] : null;
};

$loadedPackages = [];

foreach (array_keys($app->getLoadedProviders()) as $provider) {
    if ($package = $packageOf((new ReflectionClass($provider))->getFileName())) {
        $loadedPackages[$package] = true;
    }
}

$configKeysOf = function (string $class) use ($app): array {
    $keys = [];

    foreach (Illuminate\Support\Arr::dot($app['config']->all()) as $key => $value) {
        if (is_string($value) && strcasecmp(ltrim($value, '\\'), $class) === 0) {
            $keys[] = $key;
        }
    }

    return $keys;
};

foreach ($app->getBindings() as $abstract => $binding) {
    $function = new ReflectionFunction($binding['concrete']);
    $variables = $function->getStaticVariables();
    $wrapped = array_key_exists('concrete', $variables) && array_key_exists('abstract', $variables) && ! $isAppFile($function->getFileName());
    $concrete = $wrapped ? $variables['concrete'] : null;
    $closureInApp = ! $wrapped && $isAppFile($function->getFileName());

    $claimed = isset($bound[ltrim($abstract, '\\')]);
    $claimedAndWrapped = $claimed && $wrapped && is_string($concrete) && strcasecmp(ltrim($concrete, '\\'), $bound[ltrim($abstract, '\\')]['concrete'] ?? '') === 0;

    if (! $isAppClass($abstract) && ! (is_string($concrete) && $isAppClass($concrete)) && ! $closureInApp && ! $claimedAndWrapped) {
        if ($claimed) {
            $explained[ltrim($abstract, '\\')] = 'bound by the app, then bound again by package code in '.$relative($function->getFileName()).':'.$function->getStartLine();
        }

        continue;
    }

    if (! $claimed && ! $closureInApp) {
        $package = $wrapped
            ? (class_exists($abstract) || interface_exists($abstract) ? $packageOf((new ReflectionClass($abstract))->getFileName()) : null)
            : $packageOf($function->getFileName());
        $keys = $wrapped && is_string($concrete) ? $configKeysOf(ltrim($concrete, '\\')) : [];

        if (! $wrapped && $package !== null) {
            $explained[ltrim($abstract, '\\')] = "bound by package code ($package) in ".$relative($function->getFileName()).':'.$function->getStartLine();
            $actual[ltrim($abstract, '\\')] = null;

            continue;
        }

        if ($package !== null && isset($loadedPackages[$package]) && $keys !== []) {
            $explained[ltrim($abstract, '\\')] = "bound by $package to ".ltrim($concrete, '\\').', which the app\'s config names at '.implode(', ', $keys);
            $actual[ltrim($abstract, '\\')] = null;

            continue;
        }
    }

    if ($closureInApp) {
        try {
            $made = $app->make($abstract);
            $concrete = is_object($made) ? get_class($made) : get_debug_type($made);
        } catch (Throwable $exception) {
            $concrete = 'cannot be made: '.$exception->getMessage();
        }
    }

    $lifetime = in_array($abstract, $scoped, true) ? 'scoped' : ($binding['shared'] ? 'singleton' : 'bind');
    $actual[ltrim($abstract, '\\')] = [
        'concrete' => is_string($concrete) ? $canonical($concrete) : null,
        'lifetime' => $lifetime,
        'at' => $closureInApp ? $relative($function->getFileName()).':'.$function->getStartLine() : null,
    ];
}

$expected = [];

foreach ($graph['laravel']['bindings'] as $binding) {
    $expected[$binding['key']] = ['concrete' => $binding['concrete'] ?? null, 'lifetime' => $binding['lifetime'], 'at' => "{$binding['file']}:{$binding['line']}"];
}

$score('bindings', $expected, $actual, ['concrete', 'lifetime'], $explained);

// Schedule.

$actual = [];
$explained = [];

try {
    $events = $app->make(Schedule::class)->events();
} catch (Throwable $exception) {
    $failures[] = 'the schedule cannot be built here, so it is not checked: '.strtok($exception->getMessage(), "\n");
    $events = null;
}

foreach ($events ?? [] as $event) {
    $frame = $appFrame(TracingSchedule::$stacks[spl_object_id($event)] ?? []);

    if ($event instanceof CallbackEvent) {
        $job = TracingSchedule::$jobs[spl_object_id($event)] ?? null;
        $key = $job !== null ? "job $job" : 'call closure';
    } elseif (preg_match("/'?artisan'?\s+(\S+)/", $event->command, $match)) {
        $name = trim($match[1], "'\"");
        $class = isset($commands[$name]) ? get_class($commands[$name]) : null;
        $key = "command $name".($class && $isAppClass($class) ? ' → '.$canonical($class) : '');
    } else {
        $key = "exec {$event->command}";
    }

    if ($frame === null) {
        $explained[$key] = 'scheduled by package code, which the graph does not read';

        continue;
    }

    $actual[$key] = ['count' => ($actual[$key]['count'] ?? 0) + 1, 'at' => $frame['at']];
}

$expected = [];

foreach ($graph['laravel']['schedule'] as $task) {
    $key = match ($task['task']) {
        'command' => "command {$task['target']}".(isset($task['class']) && $isAppClass($task['class']) ? " → {$task['class']}" : ''),
        'job' => "job {$task['target']}",
        'call' => 'call closure',
        default => "exec {$task['target']}",
    };

    $expected[$key] = ['count' => ($expected[$key]['count'] ?? 0) + 1, 'at' => "{$task['file']}:{$task['line']}"];
}

if ($events !== null) {
    $score('schedule', $expected, $actual, ['count', 'at'], $explained);
}

// Middleware.

$httpKernel = $app->make(Illuminate\Contracts\Http\Kernel::class);
[$global, $groups, $aliases] = (fn () => [$this->middleware, $this->middlewareGroups, array_merge(property_exists($this, 'routeMiddleware') ? $this->routeMiddleware : [], property_exists($this, 'middlewareAliases') ? $this->middlewareAliases : [])])->call($httpKernel);
$slots = [['global', $global]];

foreach ($groups as $group => $middleware) {
    $slots[] = ["group:$group", $middleware];
}

foreach ($aliases as $alias => $class) {
    $slots[] = ["alias:$alias", [$class]];
}

$actual = [];

foreach ($slots as [$slot, $middleware]) {
    foreach ($middleware as $class) {
        $class = explode(':', $class, 2)[0];

        if ($isAppClass($class)) {
            $actual["$slot ".$canonical($class)] = [];
        }
    }
}

$expected = [];

foreach ($graph['laravel']['middleware'] as $fact) {
    if ($isAppClass($fact['class'])) {
        $expected["{$fact['slot']} {$fact['class']}"] = [];
    }
}

$score('middleware', $expected, $actual);

$report = ['graph' => $graphPath, 'failures' => $failures, 'sections' => $sections];

if ($reportPath) {
    file_put_contents($reportPath, json_encode($report, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE));
}

foreach ($sections as $section => $result) {
    $fields = $result['fields'] ? ', right '.implode(', ', array_map(fn ($field, $count) => "$field $count/{$result['tp']}", array_keys($result['fields']), $result['fields'])) : '';

    printf("%-10s tp=%d fp=%d fn=%d flagged=%d explained=%d%s\n", $section, $result['tp'], $result['fp'], $result['fn'], $result['flagged'], $result['explained'], $fields);
}

foreach ($failures as $failure) {
    printf("  FAILURE %s\n", $failure);
}

foreach ($sections as $section => $result) {
    foreach ($result['flagged_facts'] as $fact) {
        printf("  FLAGGED %s %s\n", $section, $fact);
    }

    foreach ($result['explained_facts'] as $fact) {
        printf("  EXPLAINED %s %s\n", $section, $fact);
    }

    foreach ($result['mismatches'] as $mismatch) {
        printf("  MISMATCH %s %s\n", $section, $mismatch);
    }
}

$mismatches = array_sum(array_map(fn ($result) => count($result['mismatches']), $sections));

exit($mismatches === 0 ? 0 : 1);
