import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DocHit } from "@/api/generated/model";
import { DOCS, UPLOAD_DOC, renderedDoc } from "@/mocks/design/search/data";
import { at, hub, searchBackend, type Route } from "@/mocks/design/search/hub";
import { json, problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";

// The Docs tab: a list of the docs until there is a query, then the sections of docs that match it, each opening the reader at
// its heading. Upload is here too (Editor and above). Every state of the States index and the design's own is covered.

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const callsTo = (calls: ReadonlyArray<{ path: string }>, prefix: string) => calls.filter((c) => c.path.startsWith(prefix)).map((c) => new URL(c.path, "http://x").searchParams);
const open = (url: string, routes: Route[] = searchBackend(), role = "viewer") => renderApp(url, hub(role, routes));
const query = () => screen.getByRole("searchbox", { name: "Docs query" });

async function table(): Promise<HTMLElement> {
  await screen.findByRole("grid", { name: "Docs" });
  await waitFor(() => expect(screen.getByRole("grid", { name: "Docs" })).not.toHaveAttribute("aria-busy", "true"));
  return screen.getByRole("grid", { name: "Docs" });
}

describe("the docs", () => {
  it("lists them until there is a query: title, path, where from, version, size and when", async () => {
    const app = open("/search?type=docs");
    const grid = await table();
    expect(callsTo(app.calls, "/api/v1/docs?")[0]?.get("limit")).toBe("100");
    expect(callsTo(app.calls, "/api/v1/docs?")[0]?.has("source")).toBe(false);
    expect(within(grid).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Title", "Path", "From", "Version", "Size", "Updated"]);
    const rows = within(grid).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    expect(rows[1]).toHaveTextContent("Instant payments");
    expect(rows[1]).toHaveTextContent("/uploads/instant-payments.md");
    expect(rows[1]).toHaveTextContent("Uploaded");
    expect(rows[1]).toHaveTextContent("v3");
    expect(rows[1]).toHaveTextContent("18 KB");
    expect(rows[0]).toHaveTextContent("Git");
    expect(screen.getByText("3 docs")).toBeInTheDocument();
    expect(within(grid).getByRole("link", { name: "Instant payments" })).toHaveAttribute("href", "/docs/view?path=%2Fuploads%2Finstant-payments.md");
  });

  it("asks for one source when it is chosen", async () => {
    const app = open("/search?type=docs");
    await table();
    await userEvent.click(screen.getByRole("radio", { name: "Git" }));
    await waitFor(() => expect(callsTo(app.calls, "/api/v1/docs?").at(-1)?.get("source")).toBe("git"));
    await waitFor(() => expect(screen.getByText("2 docs")).toBeInTheDocument());
    expect(screen.getByRole("radio", { name: "Git" })).toHaveAttribute("aria-checked", "true");
  });

  it("says there are no docs, and who can add one", async () => {
    open("/search?type=docs", [at("GET", "/api/v1/docs", () => json({ items: [], next: null }))]);
    expect(await screen.findByText("No docs yet")).toBeInTheDocument();
    expect(screen.getByText(/Docs from Git appear here once the repository is synced/)).toBeInTheDocument();
  });

  it("says what failed, with the request id, and tries again", async () => {
    let fail = true;
    open("/search?type=docs", [at("GET", "/api/v1/docs", () => (fail ? problem(500, "internal") : json({ items: DOCS, next: null })))]);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't load docs");
    expect(alert).toHaveTextContent("req-test-1");
    fail = false;
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await table();
  });

  it("opens the reader from a row", async () => {
    const app = open("/search?type=docs", [
      ...searchBackend(),
      at("GET", "/api/v1/docs/fs/render", () => json(renderedDoc(UPLOAD_DOC))),
    ]);
    const grid = await table();
    await userEvent.click(within(grid).getByRole("link", { name: "Instant payments" }));
    await waitFor(() => expect(app.location()).toBe("/docs/view?path=%2Fuploads%2Finstant-payments.md"));
    expect(await screen.findByRole("heading", { level: 1, name: "Instant payments" })).toBeInTheDocument();
  });
});

describe("a docs search", () => {
  it("asks for the best matches and shows each section: title, where from, heading path, snippet and exact place", async () => {
    const app = open("/search?type=docs&q=account%20sorting");
    const list = await screen.findByRole("list", { name: "Doc sections" });
    const asked = callsTo(app.calls, "/api/v1/docs/search")[0];
    expect(asked?.get("q")).toBe("account sorting");
    expect(asked?.get("limit")).toBe("50");
    expect(screen.getByText("5 sections in 3 docs")).toBeInTheDocument();
    expect(screen.getByText("Best matches first, from titles, headings and text")).toBeInTheDocument();
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(5);
    const flags = within(list).getByRole("link", { name: "Account sorting: Account sorting > Feature Flags" });
    expect(flags).toHaveAttribute("href", "/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md#doc-feature-flags");
    expect(flags).toHaveTextContent("Git");
    expect(flags).toHaveTextContent("account-sorting.md#doc-feature-flags");
    expect([...flags.querySelectorAll("mark")].map((m) => m.textContent?.toLowerCase())).toEqual(expect.arrayContaining(["account", "sorting"]));
    // The text between backticks is code.
    expect([...flags.querySelectorAll("code")].map((c) => c.textContent)).toContain("enableAccountSorting");
  });

  it("keeps only the sections from one source when that source is chosen, without asking again", async () => {
    const app = open("/search?type=docs&q=account%20sorting");
    await screen.findByRole("list", { name: "Doc sections" });
    await userEvent.click(screen.getByRole("radio", { name: "Git" }));
    expect(screen.getByText("4 sections in 2 docs")).toBeInTheDocument();
    expect(callsTo(app.calls, "/api/v1/docs/search")).toHaveLength(1);
    await userEvent.click(screen.getByRole("radio", { name: "Uploaded" }));
    expect(screen.getByText("1 section in 1 doc")).toBeInTheDocument();
    expect(callsTo(app.calls, "/api/v1/docs/search")[0]?.has("source")).toBe(false);
  });

  it("opens the doc at the heading the server named", async () => {
    const app = open("/search?type=docs&q=limits", [...searchBackend(), at("GET", "/api/v1/docs/fs/render", () => json(renderedDoc(UPLOAD_DOC)))]);
    await userEvent.click(await screen.findByRole("link", { name: "Instant payments: Instant payments > Limits" }));
    await waitFor(() => expect(app.location()).toBe("/docs/view?path=%2Fuploads%2Finstant-payments.md#doc-limits"));
    expect(await screen.findByText(/Limits are set per entry group/)).toBeInTheDocument();
    await waitFor(() => expect(document.getElementById("doc-limits")).toHaveAttribute("data-lk-target"));
  });

  it("opens the doc from the top when the server named no heading or one that is not a heading id", async () => {
    const hits: DocHit[] = [
      { doc: 1, title: "A", path: "/uploads/a.md", heading_path: [], snippet: "x", score: 1, related_files: [] },
      { doc: 2, title: "B", path: "/uploads/b.md", heading_path: ["B"], snippet: "x", score: 1, related_files: [], anchor: "javascript:alert(1)" },
    ];
    open("/search?type=docs&q=x", [at("GET", "/api/v1/docs/search", () => json({ hits }))]);
    const list = await screen.findByRole("list", { name: "Doc sections" });
    expect(within(list).getByRole("link", { name: /^A/ })).toHaveAttribute("href", "/docs/view?path=%2Fuploads%2Fa.md");
    expect(within(list).getByRole("link", { name: /^B/ })).toHaveAttribute("href", "/docs/view?path=%2Fuploads%2Fb.md");
  });

  it("makes no link for a hit whose path is not a doc path, and shows its text", async () => {
    const hits: DocHit[] = [{ doc: 1, title: "Odd", path: "/etc/passwd", heading_path: [], snippet: "text", score: 1, related_files: [] }];
    open("/search?type=docs&q=odd", [at("GET", "/api/v1/docs/search", () => json({ hits }))]);
    const list = await screen.findByRole("list", { name: "Doc sections" });
    expect(within(list).queryByRole("link")).toBeNull();
    expect(list).toHaveTextContent("Odd");
  });

  it("shows hidden characters and masked values in a snippet as they are", async () => {
    const hits: DocHit[] = [{ doc: 1, title: "Secrets", path: "/uploads/s.md", heading_path: ["Secrets"], snippet: "token ev‮il and [REDACTED: 20 chars, github_token]", score: 1, related_files: [] }];
    open("/search?type=docs&q=token", [at("GET", "/api/v1/docs/search", () => json({ hits }))]);
    const list = await screen.findByRole("list", { name: "Doc sections" });
    expect(list.textContent).toContain("<U+202E>");
    expect(list.textContent).not.toContain("‮");
    expect(list.textContent).toContain("[REDACTED: 20 chars, github_token]");
  });

  it("says the best matches were cut at the most the server gives", async () => {
    const hits: DocHit[] = Array.from({ length: 50 }, (_, i) => ({ doc: i + 1, title: `Doc ${i}`, path: `/uploads/d${i}.md`, heading_path: [], snippet: "word", score: 1, related_files: [] }));
    open("/search?type=docs&q=word", [at("GET", "/api/v1/docs/search", () => json({ hits }))]);
    await screen.findByRole("list", { name: "Doc sections" });
    expect(screen.getByText("Results cut off").closest("[role=status]")).toHaveTextContent("Showing the first 50 sections.");
  });

  it("puts the query in the address and asks for it, once", async () => {
    const app = open("/search?type=docs");
    await table();
    await userEvent.type(query(), "limits");
    await waitFor(() => expect(app.location()).toBe("/search?q=limits&type=docs"));
    await screen.findByRole("list", { name: "Doc sections" });
    expect(callsTo(app.calls, "/api/v1/docs/search").map((p) => p.get("q"))).toEqual(["limits"]);
  });
});

describe("every state of a docs search", () => {
  it("shows a skeleton while it runs, named and busy", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const api = hub("viewer", searchBackend());
    renderApp("/search?type=docs&q=limits", async (call) => {
      if (call.path.startsWith("/api/v1/docs/search")) await held;
      return api(call);
    });
    expect(await screen.findByLabelText("Loading doc sections")).toHaveAttribute("aria-busy", "true");
    release();
    await screen.findByRole("list", { name: "Doc sections" });
  });

  it("says what failed, with the request id, and tries again", async () => {
    let fail = true;
    open("/search?type=docs&q=limits", [at("GET", "/api/v1/docs/search", () => (fail ? problem(503, "unavailable") : json({ hits: [] })))]);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't load doc sections");
    expect(alert).toHaveTextContent("req-test-1");
    fail = false;
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No docs match “limits”")).toBeInTheDocument();
  });

  it("says nothing matched, and offers fewer words and the file text", async () => {
    const app = open("/search?type=docs&q=ledger%20sorting");
    expect(await screen.findByText("No docs match “ledger sorting”")).toBeInTheDocument();
    expect(screen.getByText(/Best matches come from titles, headings and text\./)).toBeInTheDocument();
    const box = screen.getByRole("region", { name: "Try instead" });
    expect(within(box).getByText("Use fewer words: “ledger”")).toBeInTheDocument();
    await userEvent.click(within(box).getByRole("button", { name: "Open Text" }));
    await waitFor(() => expect(app.location()).toBe("/search?q=ledger+sorting&type=text"));
  });

  it("offers every source when a source was chosen and nothing was found in it", async () => {
    open("/search?type=docs&q=instant");
    await screen.findByRole("list", { name: "Doc sections" });
    await userEvent.click(screen.getByRole("radio", { name: "Git" }));
    expect(screen.getByText("No docs match “instant”")).toBeInTheDocument();
    expect(screen.getByText(/in Git docs only/)).toBeInTheDocument();
    await userEvent.click(within(screen.getByRole("region", { name: "Try instead" })).getByRole("button", { name: "Widen" }));
    expect(await screen.findByRole("list", { name: "Doc sections" })).toBeInTheDocument();
  });
});

describe("upload", () => {
  it("is off for a Viewer, with the role that is needed", async () => {
    open("/search?type=docs");
    await table();
    const button = screen.getByRole("button", { name: "Upload doc" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription(/Editor role/);
  });

  it("lets an Editor upload a doc, and opens it", async () => {
    const created = { ...UPLOAD_DOC, id: 200, title: "Notes", path: "/uploads/notes.md" };
    const app = open(
      "/search?type=docs",
      [
        at("POST", "/api/v1/docs", () => json(created, 201)),
        at("GET", "/api/v1/docs/fs/render", () => json({ ...renderedDoc(UPLOAD_DOC), path: created.path })),
        at("GET", "/api/v1/docs", (_c, url) => json({ items: [...DOCS, created].filter((d) => d.path.startsWith(url.searchParams.get("path_prefix") ?? "")), next: null })),
      ],
      "editor",
    );
    await table();
    await userEvent.click(screen.getByRole("button", { name: "Upload doc" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Upload a doc" });
    const input = dialog.querySelector("input[type=file]") as HTMLInputElement;
    await userEvent.upload(input, new File(["# Notes\n"], "notes.md", { type: "text/markdown" }), { applyAccept: false });
    await userEvent.click(await within(dialog).findByRole("button", { name: "Upload doc" }));
    await waitFor(() => expect(app.location()).toBe("/docs/view?path=%2Fuploads%2Fnotes.md"));
    expect(app.calls.filter((c) => c.method === "POST" && c.path === "/api/v1/docs")).toHaveLength(1);
  });
});

describe("the keyboard", () => {
  it("focuses the query on /", async () => {
    open("/search?type=docs");
    await table();
    await userEvent.keyboard("/");
    await waitFor(() => expect(query()).toHaveFocus());
  });
});

describe("structure for assistive technology", () => {
  it("has no structural violation with the list of docs, with sections found, and with no match", async () => {
    const app = open("/search?type=docs");
    await table();
    await expectNoAxeViolations();
    app.router.history.push("/search?type=docs&q=account%20sorting");
    await screen.findByRole("list", { name: "Doc sections" });
    await expectNoAxeViolations();
    app.router.history.push("/search?type=docs&q=ledger%20sorting");
    await screen.findByText("No docs match “ledger sorting”");
    await expectNoAxeViolations();
  });
});
