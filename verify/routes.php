<?php

/*
 * Compares the routes in a puml graph with the routes Laravel registers when the app boots.
 *
 * usage: php verify/routes.php <graph.json> <laravel-app-root> [--json report.json]
 *
 * The route collection is swapped for one that records the call stack of every route it is given. A
 * route belongs to the app when a frame of that stack lies in one of the app's own files: a route file,
 * a service provider, bootstrap/app.php. Routes only vendor code adds belong to packages (Filament,
 * Sanctum, the framework's /up) and are listed but not scored.
 *
 * Routes are matched by methods, domain and URI, the key Laravel itself stores them under. A matched
 * route must then agree on its name, action, middleware, excluded middleware and on the file:line of
 * the innermost app frame, which is the line that registers it. An app route the graph lacks counts as
 * flagged rather than missed when the graph lists an unresolved route registration on that very line,
 * such as a package macro (`Route::feeds()`) whose routes only exist at runtime. Middleware the graph
 * writes as `runtime:…` (`config('jetstream.auth_session')`) is only known at runtime too: the route's
 * other middleware must then be among Laravel's, and the field counts as flagged.
 */

use Illuminate\Contracts\Console\Kernel;
use Illuminate\Routing\RedirectController;
use Illuminate\Routing\Route;
use Illuminate\Routing\RouteCollection;
use Illuminate\Routing\ViewController;

ini_set('display_errors', 'stderr');

[$graphPath, $root] = [$argv[1] ?? null, $argv[2] ?? null];

if (! $graphPath || ! $root) {
    fwrite(STDERR, "usage: php verify/routes.php <graph.json> <laravel-app-root> [--json report.json]\n");
    exit(2);
}

$reportPath = ($index = array_search('--json', $argv, true)) !== false ? $argv[$index + 1] : null;
$graph = json_decode(file_get_contents($graphPath), true, flags: JSON_THROW_ON_ERROR);
$root = realpath($root);

require "$root/vendor/autoload.php";

final class TracingRoutes extends RouteCollection
{
    public static array $stacks = [];

    public function add(Route $route)
    {
        self::$stacks[spl_object_id($route)] = array_values(array_filter(
            array_map(fn (array $frame) => isset($frame['file']) ? [$frame['file'], $frame['line']] : null, debug_backtrace(DEBUG_BACKTRACE_IGNORE_ARGS)),
        ));

        return parent::add($route);
    }
}

$app = require "$root/bootstrap/app.php";

if (file_exists($app->getCachedRoutesPath())) {
    fwrite(STDERR, "the routes of $root are cached; run `php artisan route:clear` first\n");
    exit(2);
}

$app->booting(fn () => $app['router']->setRoutes(new TracingRoutes));
$app->make(Kernel::class)->bootstrap();

$relative = fn (string $file) => str_starts_with($file, "$root/") ? substr($file, strlen($root) + 1) : $file;
$isAppFile = fn (string $file) => str_starts_with($file, "$root/") && ! str_starts_with($file, "$root/vendor/") && realpath($file) !== realpath(__FILE__);

$action = function (Route $route): string {
    if (($component = $route->getAction('livewire_component')) !== null) {
        return 'livewire:'.(is_object($component) ? get_class($component) : ltrim($component, '\\'));
    }

    $uses = $route->getAction('uses');

    if (! is_string($uses)) {
        return 'closure';
    }

    $uses = ltrim($uses, '\\');
    $class = explode('@', $uses)[0];

    return match ($class) {
        ViewController::class => 'view:'.$route->defaults['view'],
        RedirectController::class => 'redirect:'.$route->defaults['destination'],
        default => str_contains($uses, '@') ? $uses : "$uses@__invoke",
    };
};

$set = function (array $values): array {
    $values = array_values(array_unique(array_map(fn ($value) => is_string($value) ? ltrim($value, '\\') : get_debug_type($value), $values)));
    sort($values);

    return $values;
};

$keyOf = function (array $methods, ?string $domain, string $uri): string {
    sort($methods);

    return implode('|', $methods).' '.($domain ?? '').' '.$uri;
};

$actual = [];
$packages = [];
$framework = [];

foreach ($app['router']->getRoutes() as $route) {
    $stack = TracingRoutes::$stacks[spl_object_id($route)] ?? [];
    $appFrame = null;

    foreach ($stack as [$file, $line]) {
        if ($isAppFile($file)) {
            $appFrame = $relative($file).':'.$line;
            break;
        }
    }

    $key = $keyOf($route->methods(), $route->getDomain(), $route->uri());

    if ($appFrame === null) {
        $package = null;

        foreach ($stack as [$file]) {
            if (preg_match('#^vendor/([^/]+/[^/]+)/#', $relative($file), $match) && $match[1] !== 'laravel/framework') {
                $package = $match[1];
                break;
            }
        }

        $package === null ? $framework[] = $key : $packages[$package][] = $key;

        continue;
    }

    $actual[$key] = [
        'name' => $route->getName(),
        'action' => $action($route),
        'middleware' => $set($route->middleware()),
        'excluded_middleware' => $set($route->excludedMiddleware()),
        'at' => $appFrame,
    ];
}

$expected = [];
$duplicates = [];

foreach ($graph['laravel']['routes'] as $route) {
    $key = $keyOf($route['methods'], $route['domain'] ?? null, $route['uri']);

    if (isset($expected[$key])) {
        $duplicates[] = $key;
    }

    $expected[$key] = [
        'name' => $route['name'] ?? null,
        'action' => $route['action'],
        'middleware' => $set($route['middleware'] ?? []),
        'excluded_middleware' => $set($route['excluded_middleware'] ?? []),
        'at' => $route['file'].':'.$route['line'],
    ];
}

$flaggedLines = [];

foreach ($graph['unresolved'] as $note) {
    if ($note['kind'] === 'route') {
        $flaggedLines[$note['file'].':'.$note['line']] = $note['detail'];
    }
}

$tp = $fp = $fn = $flagged = 0;
$fields = ['name' => 0, 'action' => 0, 'middleware' => 0, 'excluded_middleware' => 0, 'at' => 0];
$flaggedFields = ['middleware' => 0, 'excluded_middleware' => 0];
$mismatches = [];

foreach ($expected as $key => $route) {
    if (! isset($actual[$key])) {
        $fp++;
        $mismatches[] = ['route' => $key, 'problem' => "not registered by Laravel (graph: {$route['at']})"];

        continue;
    }

    $tp++;

    foreach (array_keys($fields) as $field) {
        $known = isset($flaggedFields[$field]) ? array_values(array_filter($route[$field], fn ($value) => ! str_starts_with($value, 'runtime:'))) : $route[$field];

        if ($route[$field] === $actual[$key][$field]) {
            $fields[$field]++;
        } elseif ($known !== $route[$field] && array_diff($known, $actual[$key][$field]) === []) {
            $flaggedFields[$field]++;
        } else {
            $mismatches[] = ['route' => $key, 'problem' => "$field: graph ".json_encode($route[$field], JSON_UNESCAPED_SLASHES).', Laravel '.json_encode($actual[$key][$field], JSON_UNESCAPED_SLASHES)];
        }
    }
}

$flaggedRoutes = [];

foreach (array_diff_key($actual, $expected) as $key => $route) {
    if (isset($flaggedLines[$route['at']])) {
        $flagged++;
        $flaggedRoutes[] = ['route' => $key, 'at' => $route['at'], 'note' => $flaggedLines[$route['at']]];

        continue;
    }

    $fn++;
    $mismatches[] = ['route' => $key, 'problem' => "missing from graph (Laravel: {$route['at']})"];
}

foreach ($duplicates as $key) {
    $mismatches[] = ['route' => $key, 'problem' => 'registered twice in the graph'];
}

$report = [
    'graph' => $graphPath,
    'app_routes' => count($actual),
    'tp' => $tp,
    'fp' => $fp,
    'fn' => $fn,
    'flagged' => $flagged,
    'precision' => $tp + $fp ? round($tp / ($tp + $fp), 4) : 1.0,
    'recall' => $tp + $fn + $flagged ? round($tp / ($tp + $fn + $flagged), 4) : 1.0,
    'recall_of_readable' => $tp + $fn ? round($tp / ($tp + $fn), 4) : 1.0,
    'fields_correct' => $fields,
    'fields_flagged' => $flaggedFields,
    'flagged_routes' => $flaggedRoutes,
    'package_routes' => array_map('count', $packages),
    'framework_routes' => $framework,
    'mismatches' => $mismatches,
];

if ($reportPath) {
    file_put_contents($reportPath, json_encode($report, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
}

printf(
    "app routes: %d, tp=%d fp=%d fn=%d flagged=%d precision=%.4f recall=%.4f (%.4f of the routes puml does not flag)\n",
    count($actual), $tp, $fp, $fn, $flagged, $report['precision'], $report['recall'], $report['recall_of_readable'],
);
printf("matched routes with the right %s\n", implode(', ', array_map(fn ($field, $count) => "$field $count/$tp", array_keys($fields), $fields)));

if (array_sum($flaggedFields) > 0) {
    printf("matched routes whose middleware is partly only known at runtime, with the known part among Laravel's: %s\n", implode(', ', array_map(fn ($field, $count) => "$field $count", array_keys($flaggedFields), $flaggedFields)));
}
printf("not scored: %d package routes (%s), %d framework routes (%s)\n", array_sum(array_map('count', $packages)), implode(', ', array_map(fn ($package, $keys) => "$package ".count($keys), array_keys($packages), $packages)), count($framework), implode(', ', $framework));

foreach ($flaggedRoutes as $row) {
    printf("  FLAGGED %s at %s: %s\n", $row['route'], $row['at'], $row['note']);
}

foreach ($mismatches as $row) {
    printf("  MISMATCH %s: %s\n", $row['route'], $row['problem']);
}

exit(count($mismatches) === 0 ? 0 : 1);
