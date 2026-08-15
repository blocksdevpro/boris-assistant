import hljs from "highlight.js/lib/core";
import type { LanguageFn } from "highlight.js";

const LOADERS: Record<string, () => Promise<LanguageFn>> = {
    python: () => import("highlight.js/lib/languages/python").then((m) => m.default),
    rust: () => import("highlight.js/lib/languages/rust").then((m) => m.default),
    javascript: () =>
      import("highlight.js/lib/languages/javascript").then((m) => m.default),
    typescript: () =>
      import("highlight.js/lib/languages/typescript").then((m) => m.default),
    powershell: () =>
      import("highlight.js/lib/languages/powershell").then((m) => m.default),
    bash: () => import("highlight.js/lib/languages/bash").then((m) => m.default),
    json: () => import("highlight.js/lib/languages/json").then((m) => m.default),
    ini: () => import("highlight.js/lib/languages/ini").then((m) => m.default),
    yaml: () => import("highlight.js/lib/languages/yaml").then((m) => m.default),
    xml: () => import("highlight.js/lib/languages/xml").then((m) => m.default),
    css: () => import("highlight.js/lib/languages/css").then((m) => m.default),
    go: () => import("highlight.js/lib/languages/go").then((m) => m.default),
    c: () => import("highlight.js/lib/languages/c").then((m) => m.default),
    cpp: () => import("highlight.js/lib/languages/cpp").then((m) => m.default),
    java: () => import("highlight.js/lib/languages/java").then((m) => m.default),
    csharp: () => import("highlight.js/lib/languages/csharp").then((m) => m.default),
    ruby: () => import("highlight.js/lib/languages/ruby").then((m) => m.default),
    swift: () => import("highlight.js/lib/languages/swift").then((m) => m.default),
    kotlin: () => import("highlight.js/lib/languages/kotlin").then((m) => m.default),
    sql: () => import("highlight.js/lib/languages/sql").then((m) => m.default),
    markdown: () =>
      import("highlight.js/lib/languages/markdown").then((m) => m.default),
  };

const registered = new Set<string>();
const inflight = new Map<string, Promise<void>>();

async function ensureLanguage(lang: string): Promise<void> {
  if (registered.has(lang)) return;
  const pending = inflight.get(lang);
  if (pending) return pending;
  const loader = LOADERS[lang];
  if (!loader) return;
  const work = loader()
    .then((def) => {
      hljs.registerLanguage(lang, def);
      registered.add(lang);
    })
    .finally(() => {
      // A failed dynamic import must be retryable on the next render.
      inflight.delete(lang);
    });
  inflight.set(lang, work);
  await work;
}

/** Map Boris language hints onto a highlight.js grammar id. */
export function resolveHighlightLanguage(
  language: string | null | undefined,
): string | null {
  const raw = language?.trim().toLowerCase() ?? "";
  if (!raw) return null;
  switch (raw) {
    case "python":
    case "py":
      return "python";
    case "rust":
    case "rs":
      return "rust";
    case "javascript":
    case "js":
    case "node":
    case "jsx":
      return "javascript";
    case "typescript":
    case "ts":
    case "tsx":
      return "typescript";
    case "powershell":
    case "ps1":
    case "pwsh":
      return "powershell";
    case "bash":
    case "sh":
    case "shell":
    case "zsh":
      return "bash";
    case "json":
      return "json";
    case "toml":
      return "ini";
    case "yaml":
    case "yml":
      return "yaml";
    case "html":
    case "xml":
      return "xml";
    case "css":
      return "css";
    case "go":
    case "golang":
      return "go";
    case "c":
      return "c";
    case "cpp":
    case "c++":
    case "cxx":
      return "cpp";
    case "java":
      return "java";
    case "csharp":
    case "cs":
    case "c#":
      return "csharp";
    case "ruby":
    case "rb":
      return "ruby";
    case "swift":
      return "swift";
    case "kotlin":
    case "kt":
      return "kotlin";
    case "sql":
      return "sql";
    case "markdown":
    case "md":
      return "markdown";
    default:
      return null;
  }
}

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/** highlight.js HTML. Unknown / not-yet-loaded lang → escaped plain. */
export function highlightCode(
  source: string,
  language?: string | null,
): string {
  const lang = resolveHighlightLanguage(language);
  try {
    if (lang && hljs.getLanguage(lang)) {
      return hljs.highlight(source, { language: lang, ignoreIllegals: true }).value;
    }
  } catch {
    // fall through
  }
  return escapeHtml(source);
}

/** Load the grammar (if any) then highlight. */
export async function highlightCodeAsync(
  source: string,
  language?: string | null,
): Promise<string> {
  const lang = resolveHighlightLanguage(language);
  if (lang) {
    try {
      await ensureLanguage(lang);
    } catch {
      // fall through to escaped
    }
  }
  return highlightCode(source, language);
}
