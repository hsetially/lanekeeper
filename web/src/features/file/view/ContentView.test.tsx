import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { FileContent, FileEntry } from "@/api/generated/model";
import { renderFile } from "../test-utils";
import { ContentView } from "./ContentView";

const HASH = "9d2c47e" + "0".repeat(57);
const MARK = "[REDACTED: 12 chars, aws_key]";

const content = (over: Partial<FileContent> = {}): FileContent => ({
  path: "tx-infinity-api/tx-infinity-core-sit7.yml",
  hash: HASH,
  class: "structured",
  format: { eol: "crlf", encoding: "utf8", bom: false },
  size: 40,
  content_withheld: false,
  text: "a: 1\r\nb: 2\r\n",
  ...over,
});

const props = { name: "tx-infinity-core-sit7.yml", where: "NFS", highlightLine: null, entry: null, onCompareWithGit: null };

describe("ContentView: text", () => {
  it("shows the lines with their numbers, and the status bar the design has under them", () => {
    render(<ContentView {...props} content={content()} />);
    expect(screen.getAllByRole("row")).toHaveLength(2);
    const bar = screen.getByRole("group", { name: "File format" });
    expect(bar).toHaveTextContent("CRLF");
    expect(bar).toHaveTextContent("UTF-8");
    expect(bar).toHaveTextContent("YAML");
    expect(bar).toHaveTextContent("2 lines");
  });

  it("uses the masked view when the server sent masks, and marks the markers", () => {
    const text = `k: ${MARK}\n`;
    render(<ContentView {...props} content={content({ text, masks: [{ line: 1, start: 3, end: 3 + MARK.length }] })} />);
    expect(document.querySelector("[data-slot='mask']")).not.toBeNull();
  });

  it("points at the line the page names", () => {
    render(<ContentView {...props} content={content({ text: "a\nb\nc\n" })} highlightLine={2} />);
    expect(document.querySelector("[data-highlight='true']")).not.toBeNull();
  });
});

describe("ContentView: an empty file (States E03)", () => {
  it("says the file exists but has 0 bytes, and offers the comparison with Git", async () => {
    const onCompare = vi.fn();
    render(<ContentView {...props} content={content({ text: "", size: 0 })} onCompareWithGit={onCompare} />);
    expect(screen.getByText("tx-infinity-core-sit7.yml is empty on NFS")).toBeInTheDocument();
    expect(screen.getByText(/The file exists but has 0 bytes\. Compare it with Git, or restore the Git version\./)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Compare with Git" }));
    expect(onCompare).toHaveBeenCalledTimes(1);
  });

  it("offers no comparison when there is none to make", () => {
    render(<ContentView {...props} content={content({ text: "", size: 0 })} />);
    expect(screen.queryByRole("button", { name: "Compare with Git" })).not.toBeInTheDocument();
  });
});

describe("ContentView: a binary file", () => {
  it("draws an image from the blob, by its hash", () => {
    render(<ContentView {...props} name="receipt-logo.jpeg" content={content({ class: "binary", text: null, size: 18 * 1024, path: "r/receipt-logo.jpeg" })} />);
    const image = screen.getByRole("img", { name: "receipt-logo.jpeg" });
    expect(image).toHaveAttribute("src", `/api/v1/blobs/${HASH}`);
  });

  it("does not draw what is not an image the browser can show safely (an SVG can carry script)", () => {
    render(<ContentView {...props} name="logo.svg" content={content({ class: "binary", text: null, size: 100, path: "r/logo.svg" })} />);
    expect(screen.queryByRole("img", { name: "logo.svg" })).not.toBeInTheDocument();
    expect(screen.getByText("No preview for this kind of file.")).toBeInTheDocument();
  });

  it("offers the download from the blob, never from the name", () => {
    render(<ContentView {...props} name="keystore.p12" content={content({ class: "binary", text: null, size: 100, path: "r/keystore.p12" })} />);
    expect(screen.getByRole("link", { name: "Download" })).toHaveAttribute("href", `/api/v1/blobs/${HASH}`);
  });

  it("makes no image or link from a hash that is not a hash", () => {
    render(<ContentView {...props} name="a.png" content={content({ class: "binary", text: null, size: 1, hash: "../../auth/logout", path: "a.png" })} />);
    expect(screen.queryByRole("img", { name: "a.png" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Download" })).not.toBeInTheDocument();
  });
});

describe("ContentView: a file whose content is withheld", () => {
  const entry = { path: "security/keystore.jks", attribution: { source: "unknown", confidence: "low", actor: null, evidence: { observed_at: Date.UTC(2026, 9, 17, 14, 5) } } } as unknown as FileEntry;

  it("shows only what the design shows: name, size, hash, when it changed and who", async () => {
    renderFile(<ContentView {...props} name="keystore.jks" entry={entry} content={content({ class: "denied", content_withheld: true, text: null, size: 3277, path: "security/keystore.jks" })} />);
    await screen.findAllByText("keystore.jks");
    const dl = document.querySelector("dl") as HTMLElement;
    expect(within(dl).getByText("keystore.jks")).toBeInTheDocument();
    expect(within(dl).getByText("3.2 KB")).toBeInTheDocument();
    expect(within(dl).getByText("9d2c47e")).toBeInTheDocument();
    expect(within(dl).getByText("2026-10-17 14:05 UTC")).toBeInTheDocument();
    expect(within(dl).getAllByText("Unknown")).toHaveLength(2);
    expect(within(dl).getByText("found by scan")).toBeInTheDocument();
  });

  it("has no image, no download and no text for it", async () => {
    renderFile(<ContentView {...props} name="keystore.jks" entry={entry} content={content({ class: "denied", content_withheld: true, text: null, size: 3277 })} />);
    await screen.findByText("keystore.jks");
    expect(screen.queryByRole("link", { name: "Download" })).not.toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("copes with a file the listing does not know", async () => {
    renderFile(<ContentView {...props} name="keystore.jks" entry={null} content={content({ class: "denied", content_withheld: true, text: null, size: 3277 })} />);
    expect(await screen.findAllByText("Not available")).not.toHaveLength(0);
  });
});
