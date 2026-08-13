import { describe, expect, it } from "vitest";
import { parseMarkdown, splitInline } from "./renderMarkdown";

describe("parseMarkdown", () => {
  it("parses headings lists and fences", () => {
    const nodes = parseMarkdown(
      "# Title\n\n- one\n- two\n\n```rs\nfn x() {}\n```\n",
    );
    expect(nodes[0]).toEqual({ type: "h", level: 1, text: "Title" });
    expect(nodes[1]).toEqual({ type: "ul", items: ["one", "two"] });
    expect(nodes[2]).toEqual({ type: "pre", lang: "rs", text: "fn x() {}" });
  });

  it("does not treat angle brackets as html", () => {
    const nodes = parseMarkdown("use <script> tags");
    expect(nodes).toEqual([{ type: "p", text: "use <script> tags" }]);
  });
});

describe("splitInline", () => {
  it("splits code and bold", () => {
    const parts = splitInline("call `ls` **now**");
    expect(parts).toEqual([
      { t: "text", v: "call " },
      { t: "code", v: "ls" },
      { t: "text", v: " " },
      { t: "strong", v: "now" },
    ]);
  });
});
