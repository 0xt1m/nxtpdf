#!/usr/bin/env node
/**
 * Downloads a prebuilt PDFium shared library into `src-tauri/lib/`.
 *
 * PDFium is Google's PDF engine (the one inside Chrome). It is BSD-3-Clause
 * licensed and safe for commercial use, but it is a large native binary that we
 * deliberately do NOT commit to git. This script fetches the prebuilt artifact
 * published by https://github.com/bblanchon/pdfium-binaries.
 *
 * Runs automatically on `pnpm install`. Re-run by hand with `pnpm setup:pdfium`.
 * Set PDFIUM_RELEASE to pin a specific release tag, or SKIP_PDFIUM_DOWNLOAD=1
 * to bypass entirely (e.g. in a CI job that only typechecks the frontend).
 */

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, renameSync, readdirSync, statSync } from 'node:fs';
import { writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const LIB_DIR = join(ROOT, 'src-tauri', 'lib');
const REPO = 'bblanchon/pdfium-binaries';

/** Maps Node's platform/arch onto a pdfium-binaries release asset. */
const TARGETS = {
  'win32-x64': { asset: 'pdfium-win-x64.tgz', libName: 'pdfium.dll' },
  'win32-arm64': { asset: 'pdfium-win-arm64.tgz', libName: 'pdfium.dll' },
  'win32-ia32': { asset: 'pdfium-win-x86.tgz', libName: 'pdfium.dll' },
  'linux-x64': { asset: 'pdfium-linux-x64.tgz', libName: 'libpdfium.so' },
  'linux-arm64': { asset: 'pdfium-linux-arm64.tgz', libName: 'libpdfium.so' },
  'darwin-x64': { asset: 'pdfium-mac-x64.tgz', libName: 'libpdfium.dylib' },
  'darwin-arm64': { asset: 'pdfium-mac-arm64.tgz', libName: 'libpdfium.dylib' },
};

function log(msg) {
  console.log(`[pdfium] ${msg}`);
}

/** Recursively locates a file by name inside a directory tree. */
function findFile(dir, name) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      const hit = findFile(full, name);
      if (hit) return hit;
    } else if (entry === name) {
      return full;
    }
  }
  return null;
}

async function main() {
  if (process.env.SKIP_PDFIUM_DOWNLOAD === '1') {
    log('SKIP_PDFIUM_DOWNLOAD=1 — skipping.');
    return;
  }

  const key = `${process.platform}-${process.arch}`;
  const target = TARGETS[key];
  if (!target) {
    throw new Error(
      `Unsupported platform "${key}". Supported: ${Object.keys(TARGETS).join(', ')}`
    );
  }

  mkdirSync(LIB_DIR, { recursive: true });
  const destination = join(LIB_DIR, target.libName);

  if (existsSync(destination)) {
    log(`${target.libName} already present — skipping download.`);
    log('Delete it and re-run `pnpm setup:pdfium` to force a refresh.');
    return;
  }

  const tag = process.env.PDFIUM_RELEASE;
  const url = tag
    ? `https://github.com/${REPO}/releases/download/${tag}/${target.asset}`
    : `https://github.com/${REPO}/releases/latest/download/${target.asset}`;

  log(`Downloading ${target.asset} (${tag ?? 'latest'})...`);
  const response = await fetch(url, { redirect: 'follow' });
  if (!response.ok) {
    throw new Error(`Download failed: ${response.status} ${response.statusText}\n  ${url}`);
  }

  const work = join(tmpdir(), `nxtpdf-pdfium-${process.pid}`);
  mkdirSync(work, { recursive: true });

  try {
    const archive = join(work, target.asset);
    await writeFile(archive, Buffer.from(await response.arrayBuffer()));

    // `tar` ships with Windows 10 1803+ (bsdtar), macOS, and every Linux distro.
    log('Extracting...');
    execFileSync('tar', ['-xzf', archive, '-C', work], { stdio: 'inherit' });

    const extracted = findFile(work, target.libName);
    if (!extracted) {
      throw new Error(`Archive did not contain ${target.libName}`);
    }

    renameSync(extracted, destination);
    const sizeMb = (statSync(destination).size / 1024 / 1024).toFixed(1);
    log(`Installed ${target.libName} (${sizeMb} MB) -> src-tauri/lib/`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(`\n[pdfium] ERROR: ${error.message}\n`);
  console.error('NXTPDF cannot render or print PDFs without this library.');
  console.error('Retry with `pnpm setup:pdfium`, or download it manually from:');
  console.error(`  https://github.com/${REPO}/releases/latest\n`);
  process.exit(1);
});
