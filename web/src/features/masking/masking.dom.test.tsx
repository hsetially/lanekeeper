import { configure, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import type { Comparison, DiffHunk, DiffLine, FileContent, GridRow, Mask, SettingValue, TextHit } from "@/api/generated/model";
import { SanitisedDocHtml } from "@/components/app/SanitisedDocHtml";
import { DiffView } from "@/components/app/diff/DiffView";
import { MaskedTextView } from "@/components/masking/MaskedTextView";
import { CellDrawer } from "@/features/compare/grid/CellDrawer";
import { laneColumn } from "@/features/compare/grid/lanes";
import { renderFile } from "@/features/file/test-utils";
import { toasts } from "@/features/shell/toast/store";
import { at, hub } from "@/mocks/design/search/hub";
import { serveScenario } from "@/mocks/design/file/serve";
import { json, problem, renderApp } from "@/test/app";
import { RevealableMaskedText } from "./RevealableMaskedText";
import { expectNowhere, maskOf, utf8, watchConsole } from "./test-utils";

// THE MASKED-VALUES DOM TEST (plan, T5 Verify; S14b, M1, D79). Every surface that can show a value the server hid is rendered with the
// server's masks, and a planted fake secret must not be anywhere on it: not in the DOM, an attribute, a field, an accessible name, a
// title, a data attribute, the query or mutation cache, web storage, the console, the toasts, a copy button's value or a clipboard write.
//
// The secret is real to the test hub and to nobody else. The hub holds the raw file, does what the server does (replaces the value
// with `[REDACTED: N chars, rule]` and lists where, in UTF-8 byte offsets counted by the platform encoder and not by the code under
// test), and gives the raw value out in exactly one place: the audited `revealValue`. So three things are proved on every surface:
// the page was never given the value by a read; nothing on the page asks for it by itself (the hub saw no reveal); and nothing that a
// person can press or focus puts it anywhere. And the surfaces that take `masks` are shown a hub that breaks the contract (a mask over
// text that was never replaced): they must hide it, not draw it.

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 60_000 });

const SECRET = "ghp_PLANTEDfakeSECRET0123456789abcdefghij";
const SETTING = "service.clientSecret";
const MARKER = `[REDACTED: ${SECRET.length} chars, github_token]`;
const KEY = "  é😀 clientSecret";
/** The file as it is on NFS: CRLF, multi-byte characters before and after the value on its line. */
const RAW = ["service:", "  name: core", `${KEY}: ${SECRET} # 名前`, "  port: 8080", ""].join("\r\n");
const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const SWIMLANE = "sitb";

/** What the server sends for `RAW`: the value replaced by its marker, and where the marker is. */
function served(raw: string, setting: string | null = SETTING): { text: string; masks: Mask[] } {
  const text = raw.split(SECRET).join(MARKER);
  const line = text.slice(0, text.indexOf(MARKER)).split("\n").length;
  return { text, masks: [maskOf(text, line, MARKER, setting === null ? {} : { setting })] };
}
const VIEW = served(RAW);

const content = (over: Partial<FileContent> = {}): FileContent => ({
  path: PATH,
  hash: "a3f9c21".repeat(10).slice(0, 64),
  class: "structured",
  format: { eol: "crlf", encoding: "utf8", bom: false },
  size: utf8(RAW),
  content_withheld: false,
  text: VIEW.text,
  masks: VIEW.masks,
  ...over,
});

const sv = (text: string): SettingValue => ({ text, ty: "string", flag: null });
const ctx = (text: string): DiffLine => ({ kind: "context", text, inline_changes: [] });
const removed = (text: string, inline: Array<[number, number]> = []): DiffLine => ({ kind: "removed", text, inline_changes: inline.map(([start, end]) => ({ start, end })) });
const added = (text: string, inline: Array<[number, number]> = []): DiffLine => ({ kind: "added", text, inline_changes: inline.map(([start, end]) => ({ start, end })) });

/** The line of the file as a diff or a search carries it: the marker, and a changed span over the key, counted in bytes. */
const MASKED_LINE = `${KEY}: ${MARKER} # 名前`;
const KEY_SPAN: [number, number] = [utf8("  é😀 "), utf8("  é😀 clientSecret")];

const HUNKS: DiffHunk[] = [{ old_start: 1, old_lines: 4, new_start: 1, new_lines: 4, lines: [ctx("service:"), ctx("  name: core"), removed(MASKED_LINE, [KEY_SPAN]), added(MASKED_LINE.replace("github_token", "other_rule"), [KEY_SPAN]), ctx("  port: 8080")] }];

let user: ReturnType<typeof userEvent.setup>;
let writes: MockInstance<Clipboard["writeText"]>;
/** What the hub was asked to reveal. A test that did not press Reveal expects this to stay empty. */
let reveals: string[];

beforeEach(() => {
  toasts.clear();
  reveals = [];
  user = userEvent.setup({ delay: null });
  writes = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue(undefined);
});
afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(document, "cookie");
});

/** The hub gives the raw value out here and nowhere else. */
function revealRoute(call: { method: string; path: string }): Response | undefined {
  if (call.method === "POST" && call.path === `/api/v1/swimlanes/${SWIMLANE}/files/reveal`) {
    reveals.push(call.path);
    return json({ value: SECRET });
  }
  return undefined;
}

/** Focus every control in turn, with the keyboard, and hover it. */
async function walk(limit = 40): Promise<void> {
  for (let i = 0; i < limit; i++) {
    await user.tab();
    const focused = document.activeElement;
    if (focused instanceof HTMLElement && focused !== document.body) await user.hover(focused);
  }
}

/** Press every button that says it copies something. */
async function pressCopyButtons(): Promise<number> {
  const buttons = screen.queryAllByRole("button", { name: /copy/i });
  for (const button of buttons) await user.click(button);
  return buttons.length;
}

function expectClean(client: Parameters<typeof expectNowhere>[1], logged: unknown[][]): void {
  expectNowhere(SECRET, client, logged);
  expect(Object.entries(localStorage).join() + Object.entries(sessionStorage).join(), "web storage").not.toContain(SECRET);
  for (const call of writes.mock.calls) expect(call[0], "a clipboard write").not.toContain(SECRET);
  expect(reveals, "the hub was asked to reveal nothing").toEqual([]);
}

describe("the surfaces that show text the server masked", () => {
  it("the file page: the lines of the file, with the value hidden", async () => {
    const logged = watchConsole();
    const files = serveScenario("design-file-core", (call) => {
      const url = new URL(call.path, "http://hub.test");
      if (call.method === "GET" && url.pathname === `/api/v1/swimlanes/${SWIMLANE}/files/content` && (url.searchParams.get("source") ?? "nfs") === "nfs") return json(content());
      return revealRoute({ method: call.method, path: url.pathname });
    });
    const app = renderApp(`/swimlanes/${SWIMLANE}/files?path=${encodeURIComponent(PATH)}`, files);
    const table = await screen.findByRole("table", { name: "tx-infinity-core-sit7.yml" });
    // The line of the file (the setting's name is also in the token, for a screen reader, once the Reveal control is on the page).
    expect(within(table).getByText(/é😀 clientSecret:/)).toBeInTheDocument();
    expect(within(table).getByText("Hidden value")).toBeInTheDocument();
    await walk(24);
    await pressCopyButtons();
    expectClean(app.queryClient, logged);
  });

  it("the file view component, with its Reveal control: opening the dialog and closing it asks for nothing", async () => {
    const logged = watchConsole();
    const harness = renderFile(<RevealableMaskedText swimlane={SWIMLANE} path={PATH} label="core.yml" text={VIEW.text} masks={VIEW.masks} />, {
      api: (call) => revealRoute({ method: call.method, path: call.path }) ?? problem(404, "not_found"),
    });
    const button = await screen.findByRole("button", { name: `Reveal value of ${SETTING}` });
    await user.click(button);
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByLabelText(/Reason/), "checking");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await walk(10);
    expect(await pressCopyButtons()).toBe(0);
    expectClean(harness.client, logged);
  });

  it("a diff: the changed lines carry the marker, and a changed span over the key", async () => {
    const logged = watchConsole();
    const harness = renderFile(<DiffView label="Diff" hunks={HUNKS} mode="split" />);
    const rows = await screen.findAllByRole("row");
    expect(rows.some((row) => (row.textContent ?? "").includes("clientSecret"))).toBe(true);
    await walk(6);
    expectClean(harness.client, logged);
  });

  it("the compare drawer: the hunks around a hidden setting, and the note that two hidden values can be equal", async () => {
    const logged = watchConsole();
    const comparison: Comparison = {
      left: "effective:sitb",
      right: "effective:sitc",
      file: "tx-infinity-api/tx-infinity-core.yml",
      class: "structured",
      identical: false,
      eol_only: false,
      hunks: HUNKS,
      setting_changes: [{ path: SETTING, kind: "changed", left: sv(MARKER), right: sv(MARKER.replace("github_token", "other_rule")), left_location: null, right_location: null, severity: null }],
      binary: null,
    };
    const row: GridRow = {
      path: SETTING,
      cells: [
        { value: sv(MARKER), location: { path: SETTING, lines: { start: 3, end: 3 } } },
        { value: sv(MARKER.replace("github_token", "other_rule")), location: { path: SETTING, lines: { start: 3, end: 3 } } },
      ],
    };
    const harness = renderFile(
      <CellDrawer file="tx-infinity-api/tx-infinity-core.yml" channel={null} row={row} a={{ lane: laneColumn("sitb", undefined), index: 0 }} b={{ lane: laneColumn("sitc", undefined), index: 1 }} onClose={() => undefined} onSwap={() => undefined} />,
      { api: (call) => (call.path.startsWith("/api/v1/compare") ? json(comparison) : (revealRoute({ method: call.method, path: call.path }) ?? problem(404, "not_found"))) },
    );
    const dialog = await screen.findByRole("dialog", { name: "Diff at setting" });
    await screen.findByText(/A hidden value is never shown/);
    await walk(8);
    // The drawer's copy button copies the setting's name; whatever it copies, it is never the value.
    expect(await pressCopyButtons()).toBeGreaterThan(0);
    expect(within(dialog).getByText(SETTING)).toBeInTheDocument();
    expectClean(harness.client, logged);
  });

  it("search hits: the line that matched, with the marker in it", async () => {
    const logged = watchConsole();
    const hits: TextHit[] = [{ swimlane: SWIMLANE, path: PATH, line: 3, text: MASKED_LINE, ranges: [KEY_SPAN] }];
    const app = renderApp("/search?type=text&q=clientSecret", hub("viewer", [at("GET", "/api/v1/search/text", () => json({ hits, truncated: false }))]));
    const results = await screen.findByRole("tree", { name: "Results by swimlane and file" });
    expect(results.querySelector("mark")?.textContent).toBe("clientSecret");
    await walk(12);
    await pressCopyButtons();
    expectClean(app.queryClient, logged);
  });

  it("the served text of a file: the same markers, no reveal", async () => {
    const logged = watchConsole();
    const served_ = served(RAW, null);
    const harness = renderFile(<MaskedTextView label="Served core.yml" text={served_.text} masks={served_.masks} />);
    const table = await screen.findByRole("table", { name: "Served core.yml" });
    expect(table.querySelectorAll("[data-slot='hidden-value']")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: /Reveal/ })).toBeNull();
    await walk(4);
    expectClean(harness.client, logged);
  });

  it("a document: its text as lines from the first line it was asked for, and its rendered page", async () => {
    const logged = watchConsole();
    const docText = served("guide:\r\n  token: " + SECRET + "\r\n  more: 1\r\n", null);
    const harness = renderFile(
      <>
        <MaskedTextView label="Doc text" text={docText.text} masks={docText.masks} startLine={40} />
        <SanitisedDocHtml doc={{ path: "guide/intro.md", html: `<h1 id="doc-intro">Intro</h1><p>The token is ${MARKER}.</p>`, headings: [], content_hash: "a".repeat(64) }} />
      </>,
    );
    const table = await screen.findByRole("table", { name: "Doc text" });
    expect(within(table).getAllByRole("row").map((r) => within(r).getAllByRole("cell")[0]?.textContent)).toEqual(["40", "41", "42"]);
    expect(screen.getByText(/The token is/)).toBeInTheDocument();
    await walk(4);
    expectClean(harness.client, logged);
  });
});

describe("a hub that breaks the contract: a mask over text that was never replaced", () => {
  // The server was meant to put the marker where the value is. These hubs left the value in the text and still sent a mask for it
  // (or a mask that does not fit). The web cannot mask, and must not draw what a mask says is hidden.
  const masksOver = (text: string, over: Partial<Mask> = {}): Mask[] => [{ ...maskOf(text, 3, SECRET), setting: SETTING, ...over }];

  const hostile: Array<{ name: string; text: string; masks: Mask[] }> = [
    { name: "the span is the value, not a marker", text: RAW, masks: masksOver(RAW) },
    { name: "the span is the value and the mask names a setting", text: RAW, masks: masksOver(RAW, { setting: "service.other" }) },
    { name: "the span covers part of the value", text: RAW, masks: [{ ...maskOf(RAW, 3, SECRET), end: maskOf(RAW, 3, SECRET).start + 10, setting: SETTING }] },
    { name: "the span is shifted off the value by a byte", text: RAW, masks: (() => { const m = maskOf(RAW, 3, SECRET); return [{ ...m, start: m.start + 1, end: m.end + 1, setting: SETTING }]; })() },
    { name: "the span cuts a multi-byte character in two", text: RAW, masks: [{ line: 3, start: utf8(RAW.slice(0, RAW.indexOf(KEY))) + 3, end: utf8(RAW.slice(0, RAW.indexOf(KEY))) + 5 }] },
    { name: "two masks overlap the value", text: RAW, masks: [...masksOver(RAW), ...masksOver(RAW, { start: maskOf(RAW, 3, SECRET).start + 4, end: maskOf(RAW, 3, SECRET).end })] },
  ];

  it.each(hostile)("hides the line when $name", async ({ text, masks }) => {
    const logged = watchConsole();
    const harness = renderFile(<RevealableMaskedText swimlane={SWIMLANE} path={PATH} label="core.yml" text={text} masks={masks} />);
    const table = await screen.findByRole("table", { name: "core.yml" });
    expect(table.querySelector("[data-slot='hidden-line']")).not.toBeNull();
    expect(screen.queryByRole("button", { name: /Reveal/ })).toBeNull();
    // The lines around it are drawn as they are.
    expect(within(table).getByText("port: 8080")).toBeInTheDocument();
    await walk(6);
    expectClean(harness.client, logged);
  });

  const unplaceable: Array<{ name: string; masks: Mask[] }> = [
    { name: "a mask names a line the text does not have", masks: [{ ...maskOf(RAW, 3, SECRET), line: 9 }] },
    { name: "a mask names a line whose bytes it does not cover", masks: [{ ...maskOf(RAW, 3, SECRET), line: 4 }] },
    { name: "a mask names the line before the value", masks: [{ ...maskOf(RAW, 3, SECRET), line: 2 }] },
    { name: "a range runs past the end of the text", masks: [{ line: 3, start: maskOf(RAW, 3, SECRET).start, end: utf8(RAW) + 50 }] },
    { name: "an offset is not a whole number", masks: [{ ...maskOf(RAW, 3, SECRET), start: 12.5 }] },
    { name: "an offset is negative", masks: [{ ...maskOf(RAW, 3, SECRET), start: -1 }] },
    { name: "a range is backwards", masks: [{ line: 3, start: maskOf(RAW, 3, SECRET).end, end: maskOf(RAW, 3, SECRET).start }] },
    { name: "there are more masks than the contract allows", masks: Array.from({ length: 1001 }, () => maskOf(RAW, 3, SECRET)) },
  ];

  it.each(unplaceable)("refuses to draw the text at all when $name", async ({ masks }) => {
    const logged = watchConsole();
    const harness = renderFile(<RevealableMaskedText swimlane={SWIMLANE} path={PATH} label="core.yml" text={RAW} masks={masks} />);
    expect(await screen.findByText("This text can't be shown safely")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("button", { name: /Reveal/ })).toBeNull();
    expectClean(harness.client, logged);
  });

  it("holds for the served text and a doc too, which share the view", async () => {
    const logged = watchConsole();
    const harness = renderFile(
      <>
        <MaskedTextView label="Served" text={RAW} masks={masksOver(RAW, { setting: null })} />
        <MaskedTextView label="Doc" text={RAW} masks={masksOver(RAW, { setting: null })} startLine={10} />
      </>,
    );
    await screen.findAllByRole("table");
    expect(document.querySelectorAll("[data-slot='hidden-line']")).toHaveLength(2);
    expectClean(harness.client, logged);
  });
});

describe("the planted secret is reachable by one audited request, and shows only where a test says it may", () => {
  it("is shown in one span after a reveal, and is nowhere once the person has hidden it", async () => {
    const logged = watchConsole();
    const harness = renderFile(<RevealableMaskedText swimlane={SWIMLANE} path={PATH} label="core.yml" text={VIEW.text} masks={VIEW.masks} />, {
      api: (call) => revealRoute({ method: call.method, path: call.path }) ?? problem(404, "not_found"),
    });
    await user.click(await screen.findByRole("button", { name: `Reveal value of ${SETTING}` }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByLabelText(/Reason/), "checking the vault entry");
    await user.click(within(dialog).getByRole("button", { name: "Reveal value" }));
    await screen.findByText(SECRET);
    expect(reveals).toHaveLength(1);
    expectNowhere(SECRET, harness.client, logged, { inRevealedSpan: true });
    expect(await pressCopyButtons()).toBe(0);
    for (const call of writes.mock.calls) expect(call[0]).not.toContain(SECRET);
    await user.click(screen.getByRole("button", { name: `Hide value of ${SETTING}` }));
    expectNowhere(SECRET, harness.client, logged);
    harness.unmount();
    expectNowhere(SECRET, harness.client, logged);
  });
});

// A render that is not inside any provider: the pieces that take only text and masks need none.
describe("pieces that take only text and masks", () => {
  it("draw a text the same way whatever holds it", () => {
    const { container, unmount } = render(<MaskedTextView label="x" text={VIEW.text} masks={VIEW.masks} />);
    expect(container.querySelectorAll("[data-slot='hidden-value']")).toHaveLength(1);
    expect(container.innerHTML).not.toContain(SECRET);
    unmount();
  });
});
