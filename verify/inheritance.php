<?php

/*
 * Compares the inheritance edges in a puml graph against PHP's own Reflection.
 *
 * usage: php verify/inheritance.php <graph.json> <vendor/autoload.php> [--json report.json]
 *
 * Reflection only reports the full interface set, so declared interfaces are checked as:
 *   precision: every declared interface is implemented at runtime
 *   recall:    every runtime interface is explained by the parent, the declared interfaces
 *              (and their ancestors), or an engine-implicit interface (UnitEnum, BackedEnum, Stringable)
 */

[$graphPath, $autoloadPath] = [$argv[1] ?? null, $argv[2] ?? null];

if (! $graphPath || ! $autoloadPath) {
    fwrite(STDERR, "usage: php verify/inheritance.php <graph.json> <vendor/autoload.php> [--json report.json]\n");
    exit(2);
}

$reportPath = ($index = array_search('--json', $argv, true)) !== false ? $argv[$index + 1] : null;

require $autoloadPath;

error_reporting(E_ALL & ~E_DEPRECATED & ~E_USER_DEPRECATED);

$graph = json_decode(file_get_contents($graphPath), true, flags: JSON_THROW_ON_ERROR);
$classes = array_filter($graph['classes'], fn (array $class) => ! $class['external']);

$counts = [];
$mismatches = [];
$unloadable = [];
$implicit = 0;

$count = function (string $kind, string $outcome) use (&$counts) {
    $counts[$kind][$outcome] = ($counts[$kind][$outcome] ?? 0) + 1;
};

$mismatch = function (string $kind, string $class, string $problem, string $target) use (&$mismatches) {
    $mismatches[] = compact('kind', 'class', 'problem', 'target');
};

$exists = fn (string $name) => class_exists($name) || interface_exists($name) || trait_exists($name) || enum_exists($name);

foreach ($classes as $class) {
    $name = $class['id'];

    try {
        if (! $exists($name)) {
            $unloadable[] = ['class' => $name, 'reason' => 'not autoloadable'];

            continue;
        }

        $reflection = new ReflectionClass($name);
    } catch (Throwable $exception) {
        $unloadable[] = ['class' => $name, 'reason' => $exception->getMessage()];

        continue;
    }

    $declaredParent = $class['parent'] ?? null;
    $actualParent = $reflection->getParentClass() ? $reflection->getParentClass()->getName() : null;

    if ($declaredParent === null && $actualParent === null) {
        // nothing to compare
    } elseif ($declaredParent !== null && $actualParent !== null && strcasecmp($declaredParent, $actualParent) === 0) {
        $count('extends', 'tp');
    } else {
        if ($declaredParent !== null) {
            $count('extends', 'fp');
            $mismatch('extends', $name, 'not a runtime parent', $declaredParent);
        }

        if ($actualParent !== null) {
            $count('extends', 'fn');
            $mismatch('extends', $name, 'missing runtime parent', $actualParent);
        }
    }

    $declaredTraits = array_map('strtolower', $class['traits'] ?? []);
    $actualTraits = array_map('strtolower', $reflection->getTraitNames());

    foreach ($declaredTraits as $trait) {
        in_array($trait, $actualTraits, true)
            ? $count('uses-trait', 'tp')
            : [$count('uses-trait', 'fp'), $mismatch('uses-trait', $name, 'not a runtime trait', $trait)];
    }

    foreach (array_diff($actualTraits, $declaredTraits) as $trait) {
        $count('uses-trait', 'fn');
        $mismatch('uses-trait', $name, 'missing runtime trait', $trait);
    }

    $declaredInterfaces = array_map('strtolower', $class['interfaces'] ?? []);
    $actualInterfaces = array_map('strtolower', $reflection->getInterfaceNames());

    foreach ($declaredInterfaces as $interface) {
        in_array($interface, $actualInterfaces, true)
            ? $count('implements', 'tp')
            : [$count('implements', 'fp'), $mismatch('implements', $name, 'not a runtime interface', $interface)];
    }

    $explained = $declaredInterfaces;

    if ($reflection->getParentClass()) {
        $explained = [...$explained, ...array_map('strtolower', $reflection->getParentClass()->getInterfaceNames())];
    }

    foreach ($declaredInterfaces as $interface) {
        if ($exists($interface)) {
            $explained = [...$explained, ...array_map('strtolower', (new ReflectionClass($interface))->getInterfaceNames())];
        }
    }

    foreach ($reflection->getTraitNames() as $trait) {
        $explained = [...$explained, ...array_map('strtolower', (new ReflectionClass($trait))->getInterfaceNames())];
    }

    $engineImplicit = ['unitenum', 'backedenum', 'stringable'];

    foreach (array_diff($actualInterfaces, $explained) as $interface) {
        if (in_array($interface, $engineImplicit, true)) {
            $implicit++;

            continue;
        }

        $count('implements', 'fn');
        $mismatch('implements', $name, 'missing runtime interface', $interface);
    }
}

$summary = [];

foreach ($counts as $kind => $outcome) {
    $tp = $outcome['tp'] ?? 0;
    $fp = $outcome['fp'] ?? 0;
    $fn = $outcome['fn'] ?? 0;

    $summary[$kind] = [
        'tp' => $tp,
        'fp' => $fp,
        'fn' => $fn,
        'precision' => $tp + $fp ? round($tp / ($tp + $fp), 4) : 1.0,
        'recall' => $tp + $fn ? round($tp / ($tp + $fn), 4) : 1.0,
    ];
}

ksort($summary);

$report = [
    'graph' => $graphPath,
    'classes_checked' => count($classes) - count($unloadable),
    'unloadable' => count($unloadable),
    'engine_implicit_interfaces' => $implicit,
    'summary' => $summary,
    'mismatches' => $mismatches,
    'unloadable_classes' => $unloadable,
];

if ($reportPath) {
    file_put_contents($reportPath, json_encode($report, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
}

printf("classes checked: %d, unloadable: %d, engine-implicit interfaces skipped: %d\n", $report['classes_checked'], $report['unloadable'], $implicit);

foreach ($summary as $kind => $row) {
    printf("  %-11s tp=%-5d fp=%-3d fn=%-3d precision=%.4f recall=%.4f\n", $kind, $row['tp'], $row['fp'], $row['fn'], $row['precision'], $row['recall']);
}

foreach (array_slice($mismatches, 0, 50) as $row) {
    printf("  MISMATCH %-10s %s: %s %s\n", $row['kind'], $row['class'], $row['problem'], $row['target']);
}

exit(count($mismatches) === 0 ? 0 : 1);
