#!/usr/bin/env node
/**
 * Fail the production frontend build if the main entry chunk exceeds 500 KB
 * (uncompressed). Matches Vite's default warning threshold.
 */
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const LIMIT = 500 * 1024;
const assets = join(process.cwd(), "dist", "assets");
const files = readdirSync(assets).filter((f) => f.endsWith(".js"));
const oversized = [];
let matchedMainChunks = 0;
for (const file of files) {
  // Only gate the entry/main chunk, not lazy windows/highlight splits.
  if (!file.startsWith("index-") && !file.startsWith("main-")) continue;
  matchedMainChunks += 1;
  const bytes = statSync(join(assets, file)).size;
  if (bytes > LIMIT) {
    oversized.push({ file, bytes });
  }
}
if (matchedMainChunks === 0) {
  console.error("bundle-size check found no index-* or main-* entry chunk");
  process.exit(1);
}
if (oversized.length) {
  console.error("main bundle exceeds 500 KB:");
  for (const { file, bytes } of oversized) {
    console.error(`  ${file}: ${(bytes / 1024).toFixed(1)} KB`);
  }
  process.exit(1);
}
console.log(
  `bundle-size ok (${files.length} js assets, main chunks ≤ 500 KB)`,
);
