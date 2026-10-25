import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DocHit, FlagStatus } from "@/api/generated/model";
import { problem, renderFile, routes } from "../test-utils";
import { DocsPanel } from "./DocsPanel";
import { useFileDocs } from "./useFileDocs";

// "Docs for this file" (File, 1o): the docs that describe the file, with a way into the reader at the right heading, and the
// table of the feature flags the file sets, this swimlane's value next to the template default.

const FILE = "tx-infinity-api/tx-infinity-core.yml";
const hit = (over: Partial<DocHit> = {}): DocHit => ({
  doc: 12,
  title: "Account sorting",
  path: "/git/main/docs/account-sorting.md",
  heading_path: ["Account sorting", "Turning it on"],
  snippet: "Lets tellers sort an account list by balance or name. Turned on per swimlane with a feature flag in this file.",
  score: 0.9,
  related_files: [FILE],
  anchor: "doc-turning-it-on",
  ...over,
});
const flag = (over: Partial<FlagStatus> = {}): FlagStatus => ({
  flag: "enableAccountSorting",
  file: FILE,
  channel: null,
  setting: "txInfinityOptions.enableAccountSorting",
  template_default: { text: "false", ty: "bool", flag: null },
  values: [{ swimlane: "sitb", value: { text: "true", ty: "bool", flag: null } }],
  ...over,
});

const DOCS = "GET /api/v1/docs/for-file";
const FLAGS = "GET /api/v1/features/status";
const server = (docs: unknown = { hits: [hit()] }, flags: unknown = { flags: [flag()] }) => routes({ [DOCS]: () => docs, [FLAGS]: () => flags });

describe("DocsPanel", () => {
  it("asks for the docs of the logical file, and for the flags of the file in this swimlane", async () => {
    const { calls } = renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server() });
    await screen.findByText("Account sorting", { selector: "strong bdi" });
    const query = (prefix: string) => Object.fromEntries(new URL(calls.find((c) => c.path.startsWith(prefix))?.path ?? "", "http://x").searchParams);
    expect(query("/api/v1/docs/for-file")).toEqual({ file: FILE, limit: "10" });
    const flags = new URL(calls.find((c) => c.path.startsWith("/api/v1/features/status"))?.path ?? "", "http://x").searchParams;
    expect(flags.get("file")).toBe(FILE);
    expect(flags.getAll("swimlanes")).toEqual(["sitb"]);
  });

  it("is a region named for the file, and closes", async () => {
    const onClose = vi.fn();
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={onClose} />, { api: server() });
    const panel = await screen.findByRole("complementary", { name: "Docs for this file" });
    await userEvent.click(within(panel).getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows each doc with where it sits and what it says, and a link into the reader at the heading", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server() });
    const link = await screen.findByRole("link", { name: "Open the doc: Account sorting" });
    expect(link).toHaveTextContent("Open the doc");
    expect(link).toHaveAttribute("href", "/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md#doc-turning-it-on");
    expect(screen.getByText("Account sorting > Turning it on")).toBeInTheDocument();
    expect(screen.getByText(/Lets tellers sort an account list/)).toBeInTheDocument();
  });

  it("opens the doc with a route change on a plain click", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server() });
    const link = await screen.findByRole("link", { name: /Open the doc/ });
    const pushed = vi.fn((event: MouseEvent) => event.preventDefault());
    link.addEventListener("click", pushed);
    await userEvent.click(link);
    // The click is handled by the app (the default action of the anchor is cancelled), not by a page load.
    expect(pushed).toHaveBeenCalledTimes(1);
    expect(pushed.mock.calls[0]?.[0].defaultPrevented).toBe(true);
  });

  it("leaves the heading out when the server named none, or one that is not a heading id", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server({ hits: [hit({ anchor: null }), hit({ doc: 13, title: "Second", anchor: "x y" })] }) });
    const links = await screen.findAllByRole("link", { name: /Open the doc/ });
    expect(links.map((l) => l.getAttribute("href"))).toEqual(["/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md", "/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md"]);
  });

  it("makes no link of a path that is not a doc path, and says so", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server({ hits: [hit({ path: "https://evil.example/x" })] }) });
    expect(await screen.findByText("This document cannot be opened from here.")).toBeInTheDocument();
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });

  it("does not decode a percent escape in a doc path", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server({ hits: [hit({ path: "/git/main/docs/a%2e%2e/b.md", anchor: null })] }) });
    expect(await screen.findByRole("link", { name: /Open the doc/ })).toHaveAttribute("href", "/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Fa%252e%252e%2Fb.md");
  });

  it("draws a title, a heading and a snippet as text, never as markup", async () => {
    const { container } = renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, {
      api: server({ hits: [hit({ title: "<img src=x onerror=alert(1)>", heading_path: ["<b>bold</b>"], snippet: "<script>alert(1)</script> [REDACTED: 12 chars, aws_key]" })] }),
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(screen.getByText(/<script>alert\(1\)<\/script> \[REDACTED: 12 chars, aws_key\]/)).toBeInTheDocument();
    expect(container.querySelector("img, script, b")).toBeNull();
  });

  it("lists the feature flags with this swimlane's value and the template default, and marks a value that differs", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server() });
    const table = await screen.findByRole("table", { name: "Feature flags in this file" });
    expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Setting", "This file", "Default"]);
    const row = within(table).getByRole("row", { name: /txInfinityOptions\.enableAccountSorting/ });
    expect(within(row).getByText("true").parentElement).toHaveClass("font-semibold");
    expect(within(row).getByText("false").parentElement).not.toHaveClass("font-semibold");
    expect(within(row).getByText("enableAccountSorting")).toBeInTheDocument();
    expect(screen.getByText("Bold values differ from the default.")).toBeInTheDocument();
  });

  it("says Not set where there is no value, and does not bold a value that is the default", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, {
      api: server(undefined, { flags: [flag({ values: [{ swimlane: "sitb", value: { text: "false", ty: "bool", flag: null } }] }), flag({ flag: "other", setting: "a.b", template_default: null, values: [] })] }),
    });
    const table = await screen.findByRole("table");
    const first = within(table).getByRole("row", { name: /enableAccountSorting/ });
    expect(first.querySelectorAll(".font-semibold")).toHaveLength(0);
    expect(within(within(table).getByRole("row", { name: /a\.b/ })).getAllByText("Not set")).toHaveLength(2);
  });

  it("shows a masked value as its marker, and a hostile setting name as text", async () => {
    const { container } = renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, {
      api: server(undefined, { flags: [flag({ setting: "<i>x</i>", values: [{ swimlane: "sitb", value: { text: "[REDACTED: 20 chars, generic_secret]", ty: "string", flag: null } }] })] }),
    });
    expect(await screen.findByText("[REDACTED: 20 chars, generic_secret]")).toBeInTheDocument();
    expect(screen.getByText("<i>x</i>")).toBeInTheDocument();
    expect(container.querySelector("i")).toBeNull();
  });

  it("says so when no document describes the file, and when no flag is set", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server({ hits: [] }, { flags: [] }) });
    expect(await screen.findByText("No document describes this file yet.")).toBeInTheDocument();
    expect(screen.getByText("No documented feature flag is set in this file.")).toBeInTheDocument();
  });

  it("keeps the flags when the docs cannot be read, and names the problem with its request id", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: routes({ [DOCS]: () => problem(403, "forbidden"), [FLAGS]: () => ({ flags: [flag()] }) }) });
    expect(await screen.findByText("Couldn't load docs")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(await screen.findByRole("table")).toBeInTheDocument();
  });

  it("keeps the docs when the flags cannot be read", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: routes({ [DOCS]: () => ({ hits: [hit()] }), [FLAGS]: () => problem(403, "forbidden") }) });
    expect(await screen.findByText("Couldn't load feature flags")).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: /Open the doc/ })).toBeInTheDocument();
  });

  it("shows a busy region while it loads", async () => {
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: () => new Promise<Response>(() => undefined) });
    expect(await screen.findByLabelText("Loading docs")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByLabelText("Loading feature flags")).toHaveAttribute("aria-busy", "true");
  });

  it("asks for nothing until the file is known, and shows that it is waiting", async () => {
    const { calls } = renderFile(<DocsPanel lane="sitb" file={null} onClose={vi.fn()} />, { api: server() });
    expect(await screen.findByLabelText("Loading docs")).toHaveAttribute("aria-busy", "true");
    expect(calls).toHaveLength(0);
  });

  it("draws at most 200 flags and says so", async () => {
    const many = Array.from({ length: 205 }, (_, i) => flag({ flag: `f${i}`, setting: `s.f${i}` }));
    renderFile(<DocsPanel lane="sitb" file={FILE} onClose={vi.fn()} />, { api: server(undefined, { flags: many }) });
    const table = await screen.findByRole("table");
    expect(within(table).getAllByRole("row")).toHaveLength(201);
    expect(screen.getByText("Showing the first 200 flags.")).toBeInTheDocument();
  });
});

function Probe({ file }: { file: string | null }) {
  return <p>docs: {useFileDocs("sitb", file)}</p>;
}

describe("useFileDocs", () => {
  it("is available when a document describes the file", async () => {
    renderFile(<Probe file={FILE} />, { api: server({ hits: [hit()] }, { flags: [] }) });
    expect(await screen.findByText("docs: available")).toBeInTheDocument();
  });

  it("is available when only a flag is set", async () => {
    renderFile(<Probe file={FILE} />, { api: server({ hits: [] }, { flags: [flag()] }) });
    expect(await screen.findByText("docs: available")).toBeInTheDocument();
  });

  it("is none when there is nothing, and when the reads fail", async () => {
    const { unmount } = renderFile(<Probe file={FILE} />, { api: server({ hits: [] }, { flags: [] }) });
    await waitFor(() => expect(screen.getByText("docs: none")).toBeInTheDocument());
    unmount();
    renderFile(<Probe file={FILE} />, { api: routes({ [DOCS]: () => problem(403, "forbidden"), [FLAGS]: () => problem(403, "forbidden") }) });
    await waitFor(() => expect(screen.getByText("docs: none")).toBeInTheDocument());
  });

  it("asks nothing while the file is not known", async () => {
    const { calls } = renderFile(<Probe file={null} />, { api: server() });
    expect(await screen.findByText("docs: none")).toBeInTheDocument();
    expect(calls).toHaveLength(0);
  });
});
