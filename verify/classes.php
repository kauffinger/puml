<?php

/*
 * Cross-checks which class-likes a puml graph found against PHP's own tokenizer.
 *
 * usage: php verify/classes.php <graph.json> <project-root>
 */

[$graphPath, $root] = [$argv[1] ?? null, $argv[2] ?? null];

if (! $graphPath || ! $root) {
    fwrite(STDERR, "usage: php verify/classes.php <graph.json> <project-root>\n");
    exit(2);
}

$graph = json_decode(file_get_contents($graphPath), true, flags: JSON_THROW_ON_ERROR);
$composer = json_decode(file_get_contents("$root/composer.json"), true, flags: JSON_THROW_ON_ERROR);

$directories = [];

foreach ($composer['autoload']['psr-4'] ?? [] as $paths) {
    foreach ((array) $paths as $path) {
        $directories[] = realpath("$root/$path");
    }
}

$files = [];

foreach (array_filter($directories) as $directory) {
    $iterator = new RecursiveIteratorIterator(new RecursiveDirectoryIterator($directory, FilesystemIterator::SKIP_DOTS));

    foreach ($iterator as $file) {
        if ($file->getExtension() === 'php') {
            $files[$file->getRealPath()] = true;
        }
    }
}

$declared = [];

foreach (array_keys($files) as $file) {
    $tokens = PhpToken::tokenize(file_get_contents($file));
    $namespace = '';

    foreach ($tokens as $index => $token) {
        if ($token->is(T_NAMESPACE)) {
            $name = '';

            for ($next = $index + 1; isset($tokens[$next]) && ! $tokens[$next]->is([';', '{']); $next++) {
                if ($tokens[$next]->is([T_STRING, T_NAME_QUALIFIED])) {
                    $name .= $tokens[$next]->text;
                }
            }

            $namespace = $name;

            continue;
        }

        if (! $token->is([T_CLASS, T_INTERFACE, T_TRAIT, T_ENUM])) {
            continue;
        }

        $previous = $index - 1;

        while ($previous >= 0 && $tokens[$previous]->isIgnorable()) {
            $previous--;
        }

        if ($previous >= 0 && $tokens[$previous]->is([T_DOUBLE_COLON, T_NEW])) {
            continue;
        }

        $next = $index + 1;

        while (isset($tokens[$next]) && $tokens[$next]->isIgnorable()) {
            $next++;
        }

        if (isset($tokens[$next]) && $tokens[$next]->is(T_STRING)) {
            $name = ltrim("$namespace\\{$tokens[$next]->text}", '\\');
            $declared[strtolower($name)] = $name;
        }
    }
}

$found = [];

foreach ($graph['classes'] as $class) {
    if (! $class['external'] && $class['kind'] !== 'file') {
        $found[strtolower($class['id'])] = $class['id'];
    }
}

$missing = array_diff_key($declared, $found);
$extra = array_diff_key($found, $declared);

printf("files: %d, tokenizer class-likes: %d, graph class-likes: %d, missing: %d, extra: %d\n", count($files), count($declared), count($found), count($missing), count($extra));

foreach ($missing as $name) {
    echo "  MISSING $name\n";
}

foreach ($extra as $name) {
    echo "  EXTRA $name\n";
}

exit(count($missing) + count($extra) === 0 ? 0 : 1);
