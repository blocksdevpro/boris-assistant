import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const { version } = JSON.parse(readFileSync(join(root, "desktop/package.json"), "utf8"));
assert.match(version, /^\d+\.\d+\.\d+(?:-beta\.\d+)?$/);
const publicationDate = process.argv[2];
assert.ok(publicationDate, "Pass the release publication time as an ISO 8601 timestamp");
assert.match(publicationDate, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/);
const pubDate = new Date(publicationDate).toISOString();
const filename = `Boris_${version}_x64-setup.exe`;
const bundle = join(root, "target/release/bundle/nsis");
const installer = readFileSync(join(bundle, filename));
assert.equal(installer.subarray(0, 2).toString("ascii"), "MZ", "Expected a Windows installer");
const signature = readFileSync(join(bundle, `${filename}.sig`), "utf8").trim();
assert.ok(
  Buffer.from(signature, "base64").toString("utf8").startsWith("untrusted comment:"),
  "Expected a Tauri updater signature, not a key or a placeholder",
);
const notes = readFileSync(join(root, `.tauri/releases/v${version}.md`), "utf8").trim();
const manifest = {
  version,
  notes,
  pub_date: pubDate,
  platforms: {
    "windows-x86_64": {
      signature,
      url: `https://github.com/blocksdevpro/boris-assistant/releases/download/v${version}/${filename}`,
    },
  },
};
const output = join(bundle, "latest.json");
writeFileSync(output, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Created ${output}`);
console.log(`Installer SHA-256: ${createHash("sha256").update(installer).digest("hex")}`);
console.log("Verify the signature before uploading. This command does not sign or publish files.");
