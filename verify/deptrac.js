#!/usr/bin/env node
// Compares puml's rule violations with a deptrac JSON report produced from the configuration
// written by verify/deptrac-config.js: node verify/deptrac.js <graph.json> <deptrac-report.json> <project-dir>

import { readFileSync } from 'node:fs';
import { relative, resolve } from 'node:path';

const [graphPath, reportPath, projectDir] = process.argv.slice(2);

if (!projectDir) {
    console.error('usage: node verify/deptrac.js <graph.json> <deptrac-report.json> <project-dir>');
    process.exit(2);
}

const graph = JSON.parse(readFileSync(graphPath, 'utf8'));
const report = JSON.parse(readFileSync(reportPath, 'utf8'));

const puml = new Map();

for (const { rule, edge: position } of graph.verdict.violations) {
    const edge = graph.edges[position];

    for (const place of edge.evidence) {
        const key = `${place.file}:${place.line} ${edge.from} -> ${edge.to}`;
        puml.set(key, `${rule} (${edge.kind}${edge.label ? ` ${edge.label}` : ''})`);
    }
}

const deptrac = new Map();

for (const [file, entry] of Object.entries(report.files ?? {})) {
    for (const message of entry.messages) {
        if (message.type !== 'error') {
            continue;
        }

        const match = message.message.match(/^(\S+) must not depend on (\S+) \((\S+) on (\S+)\)$/);

        if (!match) {
            throw new Error(`cannot parse deptrac message: ${message.message}`);
        }

        const key = `${relative(resolve(projectDir), file)}:${message.line} ${match[1]} -> ${match[2]}`;
        deptrac.set(key, `${match[3]} on ${match[4]}`);
    }
}

const both = [...puml.keys()].filter((key) => deptrac.has(key)).sort();
const pumlOnly = [...puml.keys()].filter((key) => !deptrac.has(key)).sort();
const deptracOnly = [...deptrac.keys()].filter((key) => !puml.has(key)).sort();

const pairKey = (key) => key.slice(key.indexOf(' ') + 1);
const pumlPairs = new Set([...puml.keys()].map(pairKey));
const deptracPairs = new Set([...deptrac.keys()].map(pairKey));

console.log(`puml violations (file:line, class pair): ${puml.size}`);
console.log(`deptrac violations (file:line, class pair): ${deptrac.size}`);
console.log(`agree: ${both.length}`);
console.log(`class pairs: puml ${pumlPairs.size}, deptrac ${deptracPairs.size}, both ${[...pumlPairs].filter((pair) => deptracPairs.has(pair)).length}`);

const section = (title, keys, source) => {
    console.log(`\n${title}: ${keys.length}`);

    for (const key of keys) {
        const pairSeenByOther = (source === puml ? deptracPairs : pumlPairs).has(pairKey(key));

        console.log(`  ${key}  [${source.get(key)}]${pairSeenByOther ? '  (same class pair reported at another line)' : ''}`);
    }
};

section('only puml', pumlOnly, puml);
section('only deptrac', deptracOnly, deptrac);

process.exit(pumlOnly.length + deptracOnly.length === 0 ? 0 : 1);
