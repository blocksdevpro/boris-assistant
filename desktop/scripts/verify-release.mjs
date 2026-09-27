import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (path) => readFileSync(join(root, path), "utf8");
const pkg = JSON.parse(read("desktop/package.json"));
const lock = JSON.parse(read("desktop/package-lock.json"));
const config = JSON.parse(read("desktop/src-tauri/tauri.conf.json"));
const version = pkg.version;

assert.match(version, /^\d+\.\d+\.\d+(?:-beta\.\d+)?$/, "Unsupported release version");
for (const [source, actual] of [
  ["Tauri config", config.version],
  ["npm lockfile", lock.version],
  ["npm lockfile root package", lock.packages[""].version],
]) {
  assert.equal(actual, version, `${source} version must match desktop/package.json`);
}

// Locked metadata checks Cargo manifests and the resolved lockfile together.
const metadata = JSON.parse(execFileSync("cargo", [
  "metadata", "--format-version", "1", "--no-deps", "--locked", "--offline",
], { cwd: root, encoding: "utf8" }));
for (const name of ["boris-assistant", "boris-desktop"]) {
  const crate = metadata.packages.find((entry) => entry.name === name);
  assert.ok(crate, `Missing product crate: ${name}`);
  assert.equal(crate.version, version, `${name} version must match desktop/package.json`);
}

assert.deepEqual(
  [...config.bundle.targets].sort(),
  version.includes("-beta.") ? ["nsis"] : ["msi", "nsis"],
  "Beta releases use NSIS only; stable releases use NSIS and MSI",
);
assert.equal(config.bundle.createUpdaterArtifacts, true, "Updater artifacts must be enabled");
assert.equal(
  config.plugins.updater.pubkey.trim(),
  read(".tauri/boris.key.pub").trim(),
  "Embedded updater public key must match the checked-in public key",
);
const notes = `.tauri/releases/v${version}.md`;
assert.ok(read(notes).includes(`## Boris ${version}`), `Missing version heading in ${notes}`);
assert.ok(read("CHANGELOG.md").includes(`## [${version}]`), "Missing versioned changelog entry");

console.log(`Release metadata verified: Boris ${version} (${config.bundle.targets.join(", ")})`);
console.log("Signing and installer smoke tests remain separate release steps.");
