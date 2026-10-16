<?php

/*
 * Checks every facade edge in a puml graph against the object the booted application resolves.
 *
 * usage: php verify/facades.php <graph.json> <laravel-app-root>
 *
 * An edge is correct when Facade::getFacadeRoot() is an instance of the edge target. The target is
 * "concrete" when it is exactly the resolved class and "abstract" when it is a contract or parent.
 */

use Illuminate\Contracts\Console\Kernel;

[$graphPath, $root] = [$argv[1] ?? null, $argv[2] ?? null];

if (! $graphPath || ! $root) {
    fwrite(STDERR, "usage: php verify/facades.php <graph.json> <laravel-app-root>\n");
    exit(2);
}

$graph = json_decode(file_get_contents($graphPath), true, flags: JSON_THROW_ON_ERROR);

require "$root/vendor/autoload.php";

$app = require "$root/bootstrap/app.php";
$app->make(Kernel::class)->bootstrap();

$checked = [];
$mismatches = [];
$counts = ['concrete' => 0, 'abstract' => 0, 'wrong' => 0, 'failed' => 0];

foreach ($graph['edges'] as $edge) {
    if ($edge['kind'] !== 'facade' || isset($checked[$edge['label']])) {
        continue;
    }

    $facade = $edge['label'];
    $checked[$facade] = true;

    try {
        $resolved = $facade::getFacadeRoot();
    } catch (Throwable $exception) {
        $counts['failed']++;
        $mismatches[] = "$facade: getFacadeRoot() failed: {$exception->getMessage()}";

        continue;
    }

    $outcome = match (true) {
        ! $resolved instanceof $edge['to'] => 'wrong',
        strcasecmp(get_class($resolved), $edge['to']) === 0 => 'concrete',
        default => 'abstract',
    };

    $counts[$outcome]++;

    printf("  %-9s %s -> %s (runtime %s)\n", $outcome, $facade, $edge['to'], get_class($resolved));

    if ($outcome === 'wrong') {
        $mismatches[] = "$facade: graph says {$edge['to']}, runtime is ".get_class($resolved);
    }
}

printf("facades: %d, concrete=%d abstract=%d wrong=%d failed=%d\n", count($checked), ...array_values($counts));

foreach ($mismatches as $mismatch) {
    echo "  MISMATCH $mismatch\n";
}

exit($mismatches === [] ? 0 : 1);
