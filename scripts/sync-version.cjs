#!/usr/bin/env node

/**
 * Version Synchronization Script — redline
 *
 * Keeps the version consistent across the three sources of truth:
 *   - package.json            (source of truth)
 *   - src-tauri/Cargo.toml    ([package] version)
 *   - src-tauri/tauri.conf.json (top-level "version")
 *
 * Usage:
 *   node scripts/sync-version.cjs [version]
 *
 * With no argument, reads the version from package.json and syncs it to the
 * other two files. With an argument (e.g. 0.2.0 or 1.0.0-beta.1), sets all three.
 *
 * Version-only: this script intentionally does NOT touch productName, identifier,
 * or window title — redline has no dev/prod variant split, so there is nothing to
 * rewrite. Keep it that way unless a real variant requirement appears.
 */

const fs = require('fs');
const path = require('path');

function readJson(p) {
  return JSON.parse(fs.readFileSync(p, 'utf8'));
}

function writeJson(p, data) {
  fs.writeFileSync(p, JSON.stringify(data, null, 2) + '\n', 'utf8');
}

function updatePackageJson(p, version) {
  const pkg = readJson(p);
  pkg.version = version;
  writeJson(p, pkg);
}

function updateTauriConf(p, version) {
  const conf = readJson(p);
  conf.version = version;
  writeJson(p, conf);
}

function updateCargoToml(p, version) {
  let content = fs.readFileSync(p, 'utf8');
  // Replace only the first `version = "..."` (the [package] one, at top of file).
  content = content.replace(/^version\s*=\s*"[^"]*"/m, `version = "${version}"`);
  fs.writeFileSync(p, content, 'utf8');
}

function main() {
  const root = path.join(__dirname, '..');
  const pkgPath = path.join(root, 'package.json');

  let version = process.argv[2];
  if (!version) {
    version = readJson(pkgPath).version;
    console.log(`Reading version from package.json: ${version}`);
  } else {
    console.log(`Using provided version: ${version}`);
  }

  if (!/^\d+\.\d+\.\d+(-[a-z0-9.-]+)?$/.test(version)) {
    console.error(`Error: invalid version '${version}'. Expected X.Y.Z or X.Y.Z-suffix.`);
    process.exit(1);
  }

  const files = [
    { name: 'package.json', path: pkgPath, fn: updatePackageJson },
    { name: 'src-tauri/Cargo.toml', path: path.join(root, 'src-tauri', 'Cargo.toml'), fn: updateCargoToml },
    { name: 'src-tauri/tauri.conf.json', path: path.join(root, 'src-tauri', 'tauri.conf.json'), fn: updateTauriConf },
  ];

  let errors = 0;
  for (const f of files) {
    try {
      if (!fs.existsSync(f.path)) {
        console.warn(`  skip ${f.name} — not found`);
        continue;
      }
      f.fn(f.path, version);
      console.log(`  ok   ${f.name} -> v${version}`);
    } catch (e) {
      console.error(`  FAIL ${f.name} — ${e.message}`);
      errors++;
    }
  }

  if (errors > 0) process.exit(1);
  console.log(`Version sync complete: v${version}`);
}

if (require.main === module) main();
