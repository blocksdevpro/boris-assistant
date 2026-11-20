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

describe("parseMarkdown hard-wrapped lists", () => {
  it("joins indented continuation lines into the list item", () => {
    const nodes = parseMarkdown(
      "- first item that\n  wraps here\n- second\n\n1. step one\n   continues\n2. step two\n",
    );
    expect(nodes).toEqual([
      { type: "ul", items: ["first item that wraps here", "second"] },
      { type: "ol", items: ["step one continues", "step two"] },
    ]);
  });

  it("does not swallow an unindented paragraph after a list", () => {
    const nodes = parseMarkdown("- item\nnext paragraph");
    expect(nodes[0]).toEqual({ type: "ul", items: ["item"] });
  });
});

describe("splitInline links", () => {
  it("extracts label and href", () => {
    expect(splitInline("see [notes](https://x.dev/a) now")).toEqual([
      { t: "text", v: "see " },
      { t: "link", v: "notes", href: "https://x.dev/a" },
      { t: "text", v: " now" },
    ]);
  });
});
