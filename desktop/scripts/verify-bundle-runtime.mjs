import { access } from "node:fs/promises";
import { constants } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const targetPlatform = process.env.TAURI_ENV_PLATFORM ?? process.platform;
const isWindowsTarget = targetPlatform.includes("windows") || process.platform === "win32";

if (!isWindowsTarget) {
  process.exit(0);
}

const runtimeDir = join(scriptDir, "..", "src-tauri", "resources", "ort");
const required = ["onnxruntime.dll", "DirectML.dll"];
const missing = [];

for (const name of required) {
  try {
    await access(join(runtimeDir, name), constants.R_OK);
  } catch {
    missing.push(name);
  }
}

if (missing.length > 0) {
  throw new Error(
    `Windows bundling requires ORT runtime DLLs: missing ${missing.join(", ")}. ` +
      "Run a release Cargo build with the configured ORT backend before bundling."
  );
}
