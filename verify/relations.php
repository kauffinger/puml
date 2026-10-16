<?php

/*
 * Compares the Eloquent relations in a puml graph against `php artisan model:show --json`.
 *
 * usage: php verify/relations.php <graph.json> <laravel-app-root> [--json report.json]
 *
 * The graph records a relation on the class or trait that declares it, so the expected set for a
 * model is its own relations plus those of its traits and parents, which is what model:show sees.
 * Relations are matched by name and type. Their related class is scored separately: a morphTo target
 * is only known at runtime and is not scored, and a relation whose class the graph could not resolve
 * statically counts as a target miss.
 */

use Illuminate\Contracts\Console\Kernel;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Support\Facades\Artisan;

[$graphPath, $root] = [$argv[1] ?? null, $argv[2] ?? null];

if (! $graphPath || ! $root) {
    fwrite(STDERR, "usage: php verify/relations.php <graph.json> <laravel-app-root> [--json report.json]\n");
    exit(2);
}

$reportPath = ($index = array_search('--json', $argv, true)) !== false ? $argv[$index + 1] : null;
$graph = json_decode(file_get_contents($graphPath), true, flags: JSON_THROW_ON_ERROR);

require "$root/vendor/autoload.php";

$app = require "$root/bootstrap/app.php";
$app->make(Kernel::class)->bootstrap();

$classes = [];

foreach ($graph['classes'] as $class) {
    $classes[strtolower($class['id'])] = $class;
}

$inherited = function (string $id) use (&$inherited, $classes): array {
    $class = $classes[strtolower($id)] ?? null;

    if (! $class) {
        return [];
    }

    $relations = $class['relations'] ?? [];

    foreach ([...($class['traits'] ?? []), ...(isset($class['parent']) ? [$class['parent']] : [])] as $ancestor) {
        $relations = [...$relations, ...$inherited($ancestor)];
    }

    return $relations;
};

$key = fn (array $relation) => strtolower($relation['name']).'|'.$relation['type'];

$tp = $fp = $fn = 0;
$targets = ['correct' => 0, 'wrong' => 0, 'unresolved' => 0, 'runtime-only' => 0];
$mismatches = [];
$failures = [];
$models = 0;

foreach ($classes as $class) {
    if ($class['external'] || $class['kind'] !== 'class' || ($class['abstract'] ?? false)) {
        continue;
    }

    if (! class_exists($class['id']) || ! is_subclass_of($class['id'], Model::class)) {
        continue;
    }

    $models++;

    /*
     * model:show resolves the model from the container and inspects an empty instance. Models that
     * expose a method named like a column (laravel.io's `id(): int` returning `$this->id`) then recurse
     * through Eloquent's __get while a relation reads the parent key, so the instance gets a key.
     */
    $app->bind($class['id'], fn () => (new $class['id'])->forceFill([(new $class['id'])->getKeyName() => 1]));

    try {
        Artisan::call('model:show', ['model' => $class['id'], '--json' => true]);
        $shown = json_decode(Artisan::output(), true, flags: JSON_THROW_ON_ERROR);
    } catch (Throwable $exception) {
        $failures[] = ['model' => $class['id'], 'error' => $exception->getMessage()];

        continue;
    }

    $actual = [];

    foreach ($shown['relations'] as $relation) {
        $actual[$key($relation)] = $relation;
    }

    $expected = [];

    foreach ($inherited($class['id']) as $relation) {
        $expected[$key($relation)] = $relation;
    }

    foreach ($expected as $relationKey => $relation) {
        if (isset($actual[$relationKey])) {
            $tp++;

            $outcome = match (true) {
                $relation['type'] === 'MorphTo' => 'runtime-only',
                ! isset($relation['related']) => 'unresolved',
                strcasecmp($relation['related'], $actual[$relationKey]['related']) === 0 => 'correct',
                default => 'wrong',
            };

            $targets[$outcome]++;

            if (in_array($outcome, ['unresolved', 'wrong'], true)) {
                $mismatches[] = ['model' => $class['id'], 'problem' => "related class $outcome, model:show says {$actual[$relationKey]['related']}", 'relation' => $relationKey.'|'.($relation['related'] ?? '?')];
            }
        } else {
            $fp++;
            $mismatches[] = ['model' => $class['id'], 'problem' => 'not reported by model:show', 'relation' => $relationKey];
        }
    }

    foreach (array_diff_key($actual, $expected) as $relationKey => $relation) {
        $fn++;
        $mismatches[] = ['model' => $class['id'], 'problem' => 'missing from graph', 'relation' => $relationKey];
    }
}

$report = [
    'graph' => $graphPath,
    'models' => $models,
    'failures' => $failures,
    'tp' => $tp,
    'fp' => $fp,
    'fn' => $fn,
    'precision' => $tp + $fp ? round($tp / ($tp + $fp), 4) : 1.0,
    'recall' => $tp + $fn ? round($tp / ($tp + $fn), 4) : 1.0,
    'related_class' => $targets,
    'mismatches' => $mismatches,
];

if ($reportPath) {
    file_put_contents($reportPath, json_encode($report, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
}

printf("models: %d, model:show failures: %d, relations tp=%d fp=%d fn=%d precision=%.4f recall=%.4f\n", $models, count($failures), $tp, $fp, $fn, $report['precision'], $report['recall']);
printf("related class: correct=%d wrong=%d unresolved=%d morphTo (runtime-only, not scored)=%d\n", ...array_values($targets));

foreach ($failures as $failure) {
    printf("  FAILURE %s: %s\n", $failure['model'], $failure['error']);
}

foreach ($mismatches as $row) {
    printf("  MISMATCH %s: %s %s\n", $row['model'], $row['problem'], $row['relation']);
}

exit(count($mismatches) + count($failures) === 0 ? 0 : 1);
