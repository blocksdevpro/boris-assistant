import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import css from "highlight.js/lib/languages/css";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import markdown from "highlight.js/lib/languages/markdown";
import powershell from "highlight.js/lib/languages/powershell";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

let registered = false;

function ensureRegistered() {
  if (registered) return;
  hljs.registerLanguage("bash", bash);
  hljs.registerLanguage("c", c);
  hljs.registerLanguage("cpp", cpp);
  hljs.registerLanguage("csharp", csharp);
  hljs.registerLanguage("css", css);
  hljs.registerLanguage("go", go);
  hljs.registerLanguage("ini", ini);
  hljs.registerLanguage("java", java);
  hljs.registerLanguage("javascript", javascript);
  hljs.registerLanguage("json", json);
  hljs.registerLanguage("kotlin", kotlin);
  hljs.registerLanguage("markdown", markdown);
  hljs.registerLanguage("powershell", powershell);
  hljs.registerLanguage("python", python);
  hljs.registerLanguage("ruby", ruby);
  hljs.registerLanguage("rust", rust);
  hljs.registerLanguage("sql", sql);
  hljs.registerLanguage("swift", swift);
  hljs.registerLanguage("typescript", typescript);
  hljs.registerLanguage("xml", xml);
  hljs.registerLanguage("yaml", yaml);
  registered = true;
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

/** highlight.js HTML (escaped text + token spans). Unknown lang → escaped plain. */
export function highlightCode(
  source: string,
  language?: string | null,
): string {
  ensureRegistered();
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

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}
