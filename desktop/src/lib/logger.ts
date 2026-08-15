/**
 * Frontend logger for packaged/release builds.
 *
 * Always mirrors to the browser console. When running under Tauri, also
 * forwards lines to Rust (`frontend_log`) so they land in
 * `~/.boris/logs/boris*.log` next to the native pipeline logs.
 */

import { invoke } from "@tauri-apps/api/core";

export type LogLevel = "error" | "warn" | "info" | "debug";

function consoleFor(level: LogLevel): (...args: unknown[]) => void {
  switch (level) {
    case "error":
      return console.error.bind(console);
    case "warn":
      return console.warn.bind(console);
    case "debug":
      return console.debug.bind(console);
    default:
      return console.info.bind(console);
  }
}

function formatArg(value: unknown): string {
  if (value instanceof Error) {
    return value.stack ?? value.message;
  }
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

async function forwardToRust(
  level: LogLevel,
  message: string,
  context?: string,
): Promise<void> {
  try {
    await invoke("frontend_log", {
      level,
      message,
      context: context ?? null,
    });
  } catch {
    // Not under Tauri, or permission missing — console already has the line.
  }
}

function log(level: LogLevel, message: string, context?: unknown): void {
  const ctx =
    context === undefined || context === null
      ? undefined
      : formatArg(context);
  const line = ctx ? `${message} | ${ctx}` : message;
  consoleFor(level)(`[boris] ${line}`);
  void forwardToRust(level, message, ctx);
}

export const logger = {
  error(message: string, context?: unknown) {
    log("error", message, context);
  },
  warn(message: string, context?: unknown) {
    log("warn", message, context);
  },
  info(message: string, context?: unknown) {
    log("info", message, context);
  },
  debug(message: string, context?: unknown) {
    log("debug", message, context);
  },
};

/** Absolute path hint for the log file (may be a rolling prefix). */
export async function getLogPath(): Promise<string> {
  try {
    return await invoke<string>("get_log_path");
  } catch {
    return "";
  }
}

/** Install once: uncaught errors + unhandled rejections → file log. */
export function installGlobalErrorLogging(): void {
  window.addEventListener("error", (event) => {
    logger.error("window.error", {
      message: event.message,
      filename: event.filename,
      lineno: event.lineno,
      colno: event.colno,
      stack: event.error instanceof Error ? event.error.stack : undefined,
    });
  });

  window.addEventListener("unhandledrejection", (event) => {
    logger.error("unhandledrejection", formatArg(event.reason));
  });

  void getLogPath().then((path) => {
    if (path) logger.info("frontend logger ready", { logPath: path });
  });
}
