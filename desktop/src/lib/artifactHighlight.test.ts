import { describe, expect, it } from "vitest";
import { highlightCode, resolveHighlightLanguage } from "./artifactHighlight";

describe("resolveHighlightLanguage", () => {
  it("maps common aliases", () => {
    expect(resolveHighlightLanguage("Python")).toBe("python");
    expect(resolveHighlightLanguage("rs")).toBe("rust");
    expect(resolveHighlightLanguage("ps1")).toBe("powershell");
    expect(resolveHighlightLanguage("ts")).toBe("typescript");
    expect(resolveHighlightLanguage("toml")).toBe("ini");
    expect(resolveHighlightLanguage("nope")).toBeNull();
  });
});

describe("highlightCode", () => {
  it("colors python keywords", () => {
    const html = highlightCode("def open_bash_session():\n    pass\n", "python");
    expect(html).toContain("hljs-keyword");
    expect(html).toContain("def");
    expect(html).not.toContain("<script");
  });

  it("colors rust keywords", () => {
    const html = highlightCode("async fn main() {}", "rust");
    expect(html).toContain("hljs-keyword");
    expect(html).toContain("fn");
  });

  it("escapes unknown languages", () => {
    const html = highlightCode("<raw>", "not-a-lang");
    expect(html).toBe("&lt;raw&gt;");
  });
});
