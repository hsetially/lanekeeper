import { describe, expect, it } from "vitest";
import { MAX_UPLOAD_BYTES, formatBytes, readUpload, titleFromFileName } from "./upload";

// A doc is uploaded as JSON (`createDoc` takes a title and the Markdown text), so the browser reads the file itself. The checks
// it makes here are the dialog's states in the design (wrong type, over 2 MB, ready); the server checks again and decides.

const file = (parts: BlobPart[], name: string) => new File(parts, name, { type: "text/markdown" });

describe("readUpload", () => {
  it("reads a Markdown file as text and proposes a title from its name", async () => {
    const reading = await readUpload(file(["# Instant payments\n\nText.\n"], "instant-payments.md"));
    expect(reading).toMatchObject({ ok: true, name: "instant-payments.md", markdown: "# Instant payments\n\nText.\n", title: "Instant payments" });
    expect(reading.size).toBe(26);
  });

  it("accepts the extension in capitals", async () => {
    expect(await readUpload(file(["x"], "NOTES.MD"))).toMatchObject({ ok: true, title: "NOTES" });
  });

  it("refuses a file that is not .md, without reading it", async () => {
    const reading = await readUpload(file(["%PDF-1.7"], "retail-notes.pdf"));
    expect(reading).toMatchObject({ ok: false, problem: "wrong_type", head: "Only Markdown (.md) files are accepted." });
    expect(reading.ok ? "" : reading.message).toBe("Export the document as Markdown and choose it again.");
  });

  it("refuses a file over 2 MB by its size, without reading it", async () => {
    const big = file(["x"], "export-all-flags.md");
    Object.defineProperty(big, "size", { value: MAX_UPLOAD_BYTES + 1 });
    let read = false;
    Object.defineProperty(big, "arrayBuffer", {
      value: () => {
        read = true;
        return Promise.resolve(new ArrayBuffer(0));
      },
    });
    const reading = await readUpload(big);
    expect(reading).toMatchObject({ ok: false, problem: "too_large", head: "File is over the 2 MB limit." });
    expect(read).toBe(false);
  });

  it("accepts a file of exactly 2 MiB", async () => {
    const edge = file([new Uint8Array(MAX_UPLOAD_BYTES).fill(0x61)], "edge.md");
    expect(await readUpload(edge)).toMatchObject({ ok: true });
  });

  it("refuses an empty file", async () => {
    expect(await readUpload(file([], "empty.md"))).toMatchObject({ ok: false, problem: "empty" });
  });

  it("refuses bytes that are not UTF-8 text", async () => {
    expect(await readUpload(file([new Uint8Array([0x23, 0x20, 0xff, 0xfe, 0x0a])], "bad.md"))).toMatchObject({ ok: false, problem: "not_text" });
  });

  it("refuses a file with a NUL byte, which is not text however it is named", async () => {
    expect(await readUpload(file([new Uint8Array([0x23, 0x20, 0x00, 0x41])], "binary.md"))).toMatchObject({ ok: false, problem: "not_text" });
  });

  it("keeps the text exactly, line endings included", async () => {
    const reading = await readUpload(file(["a\r\nb\r\n"], "crlf.md"));
    expect(reading).toMatchObject({ ok: true, markdown: "a\r\nb\r\n" });
  });
});

describe("titleFromFileName", () => {
  it.each([
    ["instant-payments.md", "Instant payments"],
    ["receipts_v3.md", "Receipts v3"],
    ["account-sorting.MD", "Account sorting"],
    ["README.md", "README"],
    ["  spaced   out .md", "Spaced out"],
    ["notes.v2.md", "Notes v2"],
  ])("%s becomes %s", (name, title) => {
    expect(titleFromFileName(name)).toBe(title);
  });

  it("is never longer than a title may be", () => {
    expect(Array.from(titleFromFileName(`${"a".repeat(400)}.md`))).toHaveLength(256);
  });

  it("falls back to a word when the name leaves nothing", () => {
    expect(titleFromFileName(".md")).toBe("Untitled");
  });
});

describe("formatBytes", () => {
  it.each([
    [0, "0 B"],
    [512, "512 B"],
    [18 * 1024, "18 KB"],
    [412 * 1024, "412 KB"],
    [Math.round(3.4 * 1024 * 1024), "3.4 MB"],
    [MAX_UPLOAD_BYTES, "2.0 MB"],
  ])("%d is %s", (bytes, text) => {
    expect(formatBytes(bytes)).toBe(text);
  });
});
