#!/usr/bin/env node
/**
 * Sets the release version in the three files that must agree.
 *
 *   node scripts/set-version.mjs 1.1.0
 *
 * They have to match each other *and* the git tag. The updater compares the
 * version in `latest.json` — which comes from tauri.conf.json — against the
 * running build. Tag a release v1.1.0 while the config still says 1.0.0 and
 * the release looks fine, but nobody is ever offered it.
 */

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

const version = process.argv[2];
if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
  console.error('Usage: node scripts/set-version.mjs <major.minor.patch>');
  console.error('  e.g. node scripts/set-version.mjs 1.1.0   (no "v" prefix)');
  process.exit(1);
}

/** Rewrites a JSON file's top-level `version`, preserving formatting style. */
function setJsonVersion(relative) {
  const path = join(ROOT, relative);
  const data = JSON.parse(readFileSync(path, 'utf8'));
  const previous = data.version;
  data.version = version;
  writeFileSync(path, `${JSON.stringify(data, null, 2)}\n`);
  return previous;
}

/** Rewrites only the `[package]` version, never a dependency's. */
function setCargoVersion(relative) {
  const path = join(ROOT, relative);
  const text = readFileSync(path, 'utf8');

  const match = text.match(/^version = "(.+?)"$/m);
  if (!match) throw new Error(`No package version found in ${relative}`);

  writeFileSync(path, text.replace(/^version = ".+?"$/m, `version = "${version}"`));
  return match[1];
}

const changes = [
  ['package.json', setJsonVersion('package.json')],
  ['src-tauri/tauri.conf.json', setJsonVersion('src-tauri/tauri.conf.json')],
  ['src-tauri/Cargo.toml', setCargoVersion('src-tauri/Cargo.toml')],
];

for (const [file, previous] of changes) {
  console.log(`  ${file}: ${previous} -> ${version}`);
}

console.log(`\nNow commit, then:\n  git tag v${version}\n  git push origin v${version}`);
