import { describe, expect, it } from "vitest";
import {
  highlightCodeAsync,
  resolveHighlightLanguage,
} from "./artifactHighlight";

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
  it("colors python keywords", async () => {
    const html = await highlightCodeAsync(
      "def open_bash_session():\n    pass\n",
      "python",
    );
    expect(html).toContain("hljs-keyword");
    expect(html).toContain("def");
    expect(html).not.toContain("<script");
  });

  it("colors rust keywords", async () => {
    const html = await highlightCodeAsync("async fn main() {}", "rust");
    expect(html).toContain("hljs-keyword");
    expect(html).toContain("fn");
  });

  it("escapes unknown languages", async () => {
    const html = await highlightCodeAsync("<raw>", "not-a-lang");
    expect(html).toBe("&lt;raw&gt;");
  });
});
