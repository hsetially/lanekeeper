import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { FileContent, FileEntry } from "@/api/generated/model";
import { expectNoAxeViolations } from "@/test/axe";
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
    expect(document.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    expect(screen.getByText("Hidden value")).toBeInTheDocument();
  });

  it("gives the marker a Reveal control only when it is told the swimlane (reveal reads the file on NFS)", async () => {
    const text = `k: ${MARK}\n`;
    const masks = [{ line: 1, start: 3, end: 3 + MARK.length, setting: "k" }];
    const { unmount } = renderFile(<ContentView {...props} content={content({ text, masks })} />);
    expect(screen.queryByRole("button", { name: /Reveal value of/ })).not.toBeInTheDocument();
    unmount();
    renderFile(<ContentView {...props} lane="sitb" content={content({ text, masks })} />);
    expect(await screen.findByRole("button", { name: "Reveal value of k" })).toBeInTheDocument();
  });

  it("points at the line the page names", () => {
    render(<ContentView {...props} content={content({ text: "a\nb\nc\n" })} highlightLine={2} />);
    expect(document.querySelector("[data-highlight='true']")).not.toBeNull();
  });
});

// S14b, fail closed. The server says where it put a marker; the web believes nothing it cannot check. A mask whose span is not a
// marker, or that cannot be placed in the text, means the server and the text disagree about where the value is, so the web must
// not draw the text as if no mask had been sent. (S3's first view dropped such a mask and drew the raw line: a defect these tests
// were written against before the view was replaced.)
describe("ContentView: masks that cannot be trusted never draw the raw text (fail closed)", () => {
  const RAW = "hunter2-correct-horse";

  it("draws nothing of a line whose masked span is not a marker", () => {
    const text = `a: 1\nk: ${RAW}\nb: 2\n`;
    const start = "a: 1\n".length + 3;
    render(<ContentView {...props} content={content({ text, masks: [{ line: 2, start, end: start + RAW.length }] })} />);
    expect(document.body.textContent).not.toContain(RAW);
  });

  it("draws nothing of a text whose mask names a line the text does not have", () => {
    const text = `a: 1\nk: ${RAW}\n`;
    render(<ContentView {...props} content={content({ text, masks: [{ line: 9, start: 0, end: MARK.length }] })} />);
    expect(document.body.textContent).not.toContain(RAW);
  });

  it("draws nothing of a line whose masked span is off by one byte, so that it holds part of a marker and part of the value", () => {
    const text = `k: ${MARK}${RAW}\n`;
    render(<ContentView {...props} content={content({ text, masks: [{ line: 1, start: 4, end: 3 + MARK.length + 1 }] })} />);
    expect(document.body.textContent).not.toContain(RAW);
  });

  it("still shows the lines of the text that carry no mask, and says which line it hid", () => {
    const text = `a: 1\nk: ${RAW}\nb: 2\n`;
    const start = "a: 1\n".length + 3;
    render(<ContentView {...props} content={content({ text, masks: [{ line: 2, start, end: start + RAW.length }] })} />);
    expect(document.body.textContent).toContain("a: 1");
    expect(document.body.textContent).toContain("b: 2");
    expect(screen.getByText("Hidden line")).toBeInTheDocument();
  });

  it("says why it shows nothing when it refuses the whole text, and keeps the other chrome", () => {
    const text = `a: 1\nk: ${RAW}\n`;
    render(<ContentView {...props} content={content({ text, masks: [{ line: 9, start: 0, end: MARK.length }] })} />);
    expect(screen.getByText("This text can't be shown safely")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("holds when the Reveal control is there too: the raw value is in no reveal button name, token or live region", async () => {
    const text = `a: 1\nk: ${RAW}\n`;
    const start = "a: 1\n".length + 3;
    renderFile(<ContentView {...props} lane="sitb" content={content({ text, masks: [{ line: 2, start, end: start + RAW.length, setting: "k" }] })} />);
    await screen.findByText("Hidden line");
    expect(document.body.innerHTML).not.toContain(RAW);
    expect(screen.queryByRole("button", { name: /Reveal/ })).not.toBeInTheDocument();
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
    expect(within(dl).getByText("Unknown author")).toBeInTheDocument();
    // `unknown` carries `low` as the contract's placeholder (D73: none), so it is no score and never "Low".
    expect(within(dl).queryByText("Low")).not.toBeInTheDocument();
    expect(within(dl).getByText("Unknown")).toBeInTheDocument();
  });

  it("offers no copy for the hash of a file it will not show: nothing about it is a thing to paste elsewhere", async () => {
    renderFile(<ContentView {...props} name="keystore.jks" entry={entry} content={content({ class: "denied", content_withheld: true, text: null, size: 3277 })} />);
    await screen.findByText("keystore.jks");
    expect(screen.queryByRole("button", { name: /copy/i })).not.toBeInTheDocument();
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

  // The card is the one the page ships (FileScreen -> ContentView), so these are about it and not about a stand-in: there is nothing to
  // do with a file whose content is withheld, so the card has nothing to press, follow, type into, select or copy, and the single
  // control the page may offer is the evidence behind "Changed by".
  const FORBIDDEN_ROLES = ["link", "textbox", "combobox", "checkbox", "switch", "tab", "menuitem", "img", "table", "grid"];
  const withheld = content({ class: "denied", content_withheld: true, text: null, size: 3277, path: "security/keystore.jks" });

  it("has nothing to press, follow, type into or copy when nobody is known to have changed it", async () => {
    const { container } = renderFile(<ContentView {...props} name="keystore.jks" entry={null} content={withheld} />);
    await screen.findByText("keystore.jks");
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    for (const role of FORBIDDEN_ROLES) expect(screen.queryAllByRole(role), role).toHaveLength(0);
    expect(container.querySelector("a, input, textarea, select, [contenteditable], [download], pre, code, [data-slot='virtual-list']")).toBeNull();
  });

  it("has, with an author, the one control that opens the evidence, and nothing that copies", async () => {
    renderFile(<ContentView {...props} name="keystore.jks" entry={entry} content={withheld} />);
    await screen.findByText("keystore.jks");
    const buttons = screen.getAllByRole("button");
    expect(buttons).toHaveLength(1);
    expect(buttons[0]).toHaveAttribute("aria-haspopup");
    expect(screen.queryByRole("button", { name: /copy/i })).toBeNull();
    for (const role of FORBIDDEN_ROLES) expect(screen.queryAllByRole(role), role).toHaveLength(0);
  });

  it("draws hidden characters in the name as visible markers", async () => {
    renderFile(<ContentView {...props} name={"a\u202eb.jks"} entry={null} content={withheld} />);
    expect(await screen.findByText("<U+202E>")).toBeInTheDocument();
  });

  it("has no axe violations, with an author and without", async () => {
    const { container, unmount } = renderFile(<ContentView {...props} name="keystore.jks" entry={entry} content={withheld} />);
    await screen.findByText("keystore.jks");
    await expectNoAxeViolations(container);
    unmount();
    const bare = renderFile(<ContentView {...props} name="keystore.jks" entry={null} content={withheld} />);
    await screen.findByText("keystore.jks");
    await expectNoAxeViolations(bare.container);
  });
});
