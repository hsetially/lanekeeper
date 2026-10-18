import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import type { DocSummary, RenderedDoc } from "@/api/generated/model";
import { DOCS, GIT_DOC, UPLOAD_DOC, renderedDoc } from "@/mocks/design/search/data";
import { asked, at, hub, type Route } from "@/mocks/design/search/hub";
import { json, problem, renderApp } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";
import { slugFromDocPath } from "./doc-path";

// The doc reader (`/docs/view?path=`): who may replace or delete, what the page makes of the server's HTML, where a link from a
// search lands, and every state the States index lists for a screen that reads one thing (loading, empty, error).

// The first test of a file also loads the app's chunks, which can take longer than the default second.
configure({ asyncUtilTimeout: 5000 });

const viewUrl = (path: string, hash = "") => `/docs/view?path=${encodeURIComponent(path)}${hash}`;

type Options = { docs?: readonly DocSummary[]; page?: (doc: DocSummary) => Response | RenderedDoc; list?: () => Response };

function routes({ docs = DOCS, page, list }: Options = {}): Route[] {
  return [
    at("GET", "/api/v1/docs", (_call, url) => {
      if (list) return list();
      const prefix = url.searchParams.get("path_prefix") ?? "";
      return json({ items: docs.filter((d) => d.path.startsWith(prefix)), next: null });
    }),
    at("GET", "/api/v1/docs/fs/render", (_call, url) => {
      const doc = docs.find((d) => d.path === url.searchParams.get("path"));
      if (!doc) return problem(404, "not_found");
      const made = page ? page(doc) : renderedDoc(doc);
      return made instanceof Response ? made : json(made);
    }),
  ];
}

let scroll: MockInstance<Element["scrollIntoView"]>;
beforeEach(() => {
  scroll = vi.spyOn(Element.prototype, "scrollIntoView");
});
afterEach(() => {
  scroll.mockRestore();
  Reflect.deleteProperty(document, "cookie");
});

describe("a doc", () => {
  it("shows its title as the page's one h1, the page the server rendered and the table of contents", async () => {
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes()));
    expect(await screen.findByRole("heading", { level: 1, name: "Account sorting" })).toBeInTheDocument();
    expect(await screen.findByText(/orders the account list returned to/)).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    const toc = screen.getByRole("navigation", { name: "Table of contents" });
    const links = within(toc).getAllByRole("link");
    expect(links.map((a) => a.textContent)).toEqual(["Overview", "Feature Flags", "Rollout", "Troubleshooting"]);
    expect(links.map((a) => a.getAttribute("href"))).toEqual(["#doc-overview", "#doc-feature-flags", "#doc-rollout", "#doc-troubleshooting"]);
  });

  it("still has one h1 when the doc has a heading of level 1 of its own (the server's allowlist lets `h1` through)", async () => {
    const html = '<h1 id="doc-account-sorting">Account sorting, as the document names it</h1><p>It orders accounts.</p><h2 id="doc-overview">Overview</h2><p>text</p>';
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes({ page: (doc) => ({ ...renderedDoc(doc), html }) })));
    expect(await screen.findByText("It orders accounts.")).toBeInTheDocument();
    // The page's title is the one h1; the doc's own title is still on the page, a level down, and its sections go down with it.
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Account sorting");
    expect(screen.getByRole("heading", { level: 2, name: "Account sorting, as the document names it" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 3, name: "Overview" })).toBeInTheDocument();
    // The ids are the server's, so the table of contents and a link to a section still find their heading.
    expect(document.getElementById("doc-overview")?.tagName).toBe("H3");
  });

  it("makes the text of the doc a scroll region the keyboard can reach (axe `scrollable-region-focusable`)", async () => {
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes()));
    await screen.findByText(/orders the account list returned to/);
    // The page scrolls inside this region and the doc may have no link in it, so the region takes focus itself, and is named.
    const article = screen.getByRole("article", { name: "Document text" });
    expect(article).toHaveAttribute("tabindex", "0");
  });

  it("puts the file's name in the h1 at once, before anything has arrived", async () => {
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes()));
    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent(/account-sorting\.md|Account sorting/);
  });

  it("asks which doc has the path, then for its page by the same path", async () => {
    const app = renderApp(viewUrl(UPLOAD_DOC.path), hub("viewer", routes()));
    await screen.findByText(/Instant payments settle through/);
    const calls = asked(app.calls);
    expect(calls[0]).toContain("GET /api/v1/docs?");
    expect(new URL(calls[0]?.split(" ")[1] ?? "", "http://x").searchParams.get("path_prefix")).toBe(UPLOAD_DOC.path);
    expect(calls.some((c) => c.startsWith("GET /api/v1/docs/fs/render?") && new URL(c.split(" ")[1] ?? "", "http://x").searchParams.get("path") === UPLOAD_DOC.path)).toBe(true);
  });

  it("opens at the heading the address names, and marks it", async () => {
    renderApp(viewUrl(GIT_DOC.path, "#doc-feature-flags"), hub("viewer", routes()));
    await screen.findByText(/Each channel reads its own flag/);
    await waitFor(() => expect(document.getElementById("doc-feature-flags")).toHaveAttribute("data-lk-target"));
    expect(scroll.mock.contexts).toContain(document.getElementById("doc-feature-flags"));
    const toc = screen.getByRole("navigation", { name: "Table of contents" });
    expect(within(toc).getByRole("link", { name: "Feature Flags" })).toHaveAttribute("aria-current", "location");
  });

  it("ignores an address that names no heading of the page, and one that is not shaped like a heading id", async () => {
    renderApp(viewUrl(GIT_DOC.path, "#nothing-here"), hub("viewer", routes()));
    await screen.findByText(/Each channel reads its own flag/);
    expect(scroll).not.toHaveBeenCalled();
    expect(document.querySelector("[data-lk-target]")).toBeNull();
  });

  it("lets the person copy a link to the doc, and, once a heading is chosen, a link to that section", async () => {
    const user = userEvent.setup();
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes()));
    await screen.findByText(/Each channel reads its own flag/);
    expect(screen.queryByRole("button", { name: "Copy link to this section" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Copy link to this doc" }));
    expect(await navigator.clipboard.readText()).toBe(`${window.location.origin}${viewUrl(GIT_DOC.path)}`);
    await user.click(within(screen.getByRole("navigation", { name: "Table of contents" })).getByRole("link", { name: "Rollout" }));
    await user.click(await screen.findByRole("button", { name: "Copy link to this section" }));
    expect(await navigator.clipboard.readText()).toBe(`${window.location.origin}${viewUrl(GIT_DOC.path, "#doc-rollout")}`);
  });

  it("moves to a heading and marks it when its entry in the table of contents is used", async () => {
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes()));
    await screen.findByText(/Each channel reads its own flag/);
    await userEvent.click(within(screen.getByRole("navigation", { name: "Table of contents" })).getByRole("link", { name: "Troubleshooting" }));
    expect(document.getElementById("doc-troubleshooting")).toHaveAttribute("data-lk-target");
    expect(document.getElementById("doc-feature-flags")).not.toHaveAttribute("data-lk-target");
    expect(scroll.mock.contexts).toContain(document.getElementById("doc-troubleshooting"));
  });

  it("never lets the browser fetch an image that a doc names: it shows the alt text", async () => {
    const page = (doc: DocSummary): RenderedDoc => ({ ...renderedDoc(doc), html: '<h2 id="doc-overview">Overview</h2><p>See <img src="/api/v1/events" alt="the picture"> here.</p>' });
    const { container } = renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes({ page })));
    await screen.findByText(/See the picture here\./);
    expect(container.querySelector("img")).toBeNull();
  });

  it("hands a link in the page to the browser as the server wrote it", async () => {
    const page = (doc: DocSummary): RenderedDoc => ({ ...renderedDoc(doc), html: '<p><a href="/%2F%2Fevil.example/" rel="noopener noreferrer">odd</a></p>' });
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes({ page })));
    expect(await screen.findByRole("link", { name: "odd" })).toHaveAttribute("href", "/%2F%2Fevil.example/");
  });
});

describe("who may change a doc", () => {
  it("makes a doc from Git read-only: it says so and offers no way round it", async () => {
    renderApp(viewUrl(GIT_DOC.path), hub("editor", routes()));
    const note = await screen.findByRole("note");
    expect(note).toHaveTextContent("Read-only");
    expect(note).toHaveTextContent("managed in Git");
    const replace = screen.getByRole("button", { name: "Replace" });
    expect(replace).toHaveAttribute("aria-disabled", "true");
    expect(replace).toHaveAccessibleDescription(/managed in Git/);
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("disables Replace and Delete for a Viewer, with the role they need (not hidden)", async () => {
    renderApp(viewUrl(UPLOAD_DOC.path), hub("viewer", routes()));
    await screen.findByText(/Instant payments settle through/);
    for (const name of ["Replace", "Delete"]) {
      const button = screen.getByRole("button", { name });
      expect(button).toHaveAttribute("aria-disabled", "true");
      expect(button).toHaveAccessibleDescription(/Editor role/);
    }
    expect(screen.queryByRole("note")).toBeNull();
  });

  it("lets an Editor open Replace and Delete on an uploaded doc", async () => {
    renderApp(viewUrl(UPLOAD_DOC.path), hub("editor", routes()));
    await screen.findByText(/Instant payments settle through/);
    expect(screen.getByRole("button", { name: "Replace" })).not.toHaveAttribute("aria-disabled");
    expect(screen.getByRole("button", { name: "Delete" })).not.toHaveAttribute("aria-disabled");
  });

  it("describes where the doc comes from", async () => {
    renderApp(viewUrl(UPLOAD_DOC.path), hub("viewer", routes()));
    const aside = await screen.findByRole("complementary", { name: "Doc details" });
    expect(within(aside).getByText("Uploaded")).toBeInTheDocument();
    expect(aside).toHaveTextContent("Version 3");
    expect(aside).toHaveTextContent("18 KB");
  });
});

describe("changing a doc from the reader", () => {
  const choose = (text: string) => userEvent.upload(document.querySelector<HTMLInputElement>("input[type=file]") as HTMLInputElement, new File([text], "instant-payments.md", { type: "text/markdown" }), { applyAccept: false });

  it("replaces the text, and the reader shows the new version without a reload", async () => {
    let current: DocSummary = UPLOAD_DOC;
    let html = renderedDoc(UPLOAD_DOC).html;
    const api = hub("editor", [
      at("PUT", "/api/v1/docs/102", (call) => {
        const body = JSON.parse(call.body ?? "{}") as { markdown: string; expected_version: number };
        if (body.expected_version !== current.version) return problem(409, "conflict");
        current = { ...current, version: current.version + 1 };
        html = `<h2 id="doc-overview">Overview</h2><p>${body.markdown.trim()}</p>`;
        return json(current);
      }),
      at("GET", "/api/v1/docs", () => json({ items: [current], next: null })),
      at("GET", "/api/v1/docs/fs/render", () => json({ ...renderedDoc(UPLOAD_DOC), html })),
    ]);
    const app = renderApp(viewUrl(UPLOAD_DOC.path), api);
    await screen.findByText(/Instant payments settle through/);
    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    await choose("The rail now settles at once.\n");
    await userEvent.click(await screen.findByRole("button", { name: "Replace doc" }));
    expect(await screen.findByText("The rail now settles at once.")).toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "Doc details" })).toHaveTextContent("Version 4");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(app.calls.filter((c) => c.method === "PUT")).toHaveLength(1);
  });

  it("deletes the doc after the name is typed, and goes back to Docs", async () => {
    let exists = true;
    const api = hub("editor", [
      at("DELETE", "/api/v1/docs/102", () => {
        exists = false;
        return new Response(null, { status: 204 });
      }),
      at("GET", "/api/v1/docs", () => json({ items: exists ? [UPLOAD_DOC] : [], next: null })),
      at("GET", "/api/v1/docs/fs/render", () => json(renderedDoc(UPLOAD_DOC))),
    ]);
    const app = renderApp(viewUrl(UPLOAD_DOC.path), api);
    await screen.findByText(/Instant payments settle through/);
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete “Instant payments”?" });
    await userEvent.type(within(dialog).getByLabelText(/Type instant-payments\.md to confirm/), "instant-payments.md");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete doc" }));
    await waitFor(() => expect(app.location()).toBe("/search?type=docs"));
    expect(app.calls.filter((c) => c.method === "DELETE")).toHaveLength(1);
  });

  it("tells the Editor when someone else replaced the doc first, and keeps the dialog open", async () => {
    const api = hub("editor", [
      at("PUT", "/api/v1/docs/102", () => problem(409, "conflict")),
      ...routes(),
    ]);
    renderApp(viewUrl(UPLOAD_DOC.path), api);
    await screen.findByText(/Instant payments settle through/);
    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    await choose("New text\n");
    await userEvent.click(await screen.findByRole("button", { name: "Replace doc" }));
    expect(await screen.findByText("Someone else changed it first")).toBeInTheDocument();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });
});

describe("every state a reader can be in", () => {
  it("shows a skeleton while the doc loads, named and busy", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const api = hub("viewer", routes());
    renderApp(viewUrl(GIT_DOC.path), async (call) => {
      if (call.path.startsWith("/api/v1/docs")) await held;
      return api(call);
    });
    expect(await screen.findByLabelText("Loading doc")).toHaveAttribute("aria-busy", "true");
    release();
    expect(await screen.findByText(/orders the account list returned to/)).toBeInTheDocument();
  });

  it("says a doc was not found when no doc has the path, and offers the way back to Docs", async () => {
    renderApp(viewUrl("/uploads/gone.md"), hub("viewer", routes()));
    expect(await screen.findByText("This doc was not found")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to Docs" })).toHaveAttribute("href", "/search?type=docs");
  });

  it("shows the problem's title and the request id when the list fails, and retries", async () => {
    let fail = true;
    const list = () => (fail ? problem(500, "internal") : json({ items: DOCS, next: null }));
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes({ list })));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't load doc");
    expect(alert).toHaveTextContent("req-test-1");
    fail = false;
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(await screen.findByText(/orders the account list returned to/)).toBeInTheDocument();
  });

  it("explains a page the server refuses to render because it is too large", async () => {
    const page = () => problem(413, "payload_too_large");
    renderApp(viewUrl(GIT_DOC.path), hub("viewer", routes({ page })));
    expect(await screen.findByText("This doc is too large to show")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });

  it("does not ask the server about an address that is not a doc path", async () => {
    const app = renderApp(viewUrl("/etc/passwd"), hub("viewer", routes()));
    expect(await screen.findByText("That address is not a doc")).toBeInTheDocument();
    expect(asked(app.calls)).toEqual([]);
  });

  it("does not ask when the address names no doc at all", async () => {
    const app = renderApp("/docs/view", hub("viewer", routes()));
    expect(await screen.findByText("That address is not a doc")).toBeInTheDocument();
    expect(asked(app.calls)).toEqual([]);
  });
});

describe("/docs/:slug", () => {
  it("opens the doc whose path the slug encodes", async () => {
    renderApp(`/docs/${slugFromDocPath(UPLOAD_DOC.path)}`, hub("viewer", routes()));
    expect(await screen.findByRole("heading", { level: 1, name: "Instant payments" })).toBeInTheDocument();
    expect(await screen.findByText(/Instant payments settle through/)).toBeInTheDocument();
  });

  it("is not a doc when the slug is a word, as the handoff's example writes it", async () => {
    const app = renderApp("/docs/account-sorting", hub("viewer", routes()));
    expect(await screen.findByText("That address is not a doc")).toBeInTheDocument();
    expect(asked(app.calls)).toEqual([]);
  });
});

describe("structure for assistive technology", () => {
  it("has no structural violation for a Git doc, an uploaded doc, and the replace and delete dialogs", async () => {
    const app = renderApp(viewUrl(GIT_DOC.path), hub("editor", routes()));
    await screen.findByText(/Each channel reads its own flag/);
    await expectNoAxeViolations();
    app.router.history.push(viewUrl(UPLOAD_DOC.path));
    await screen.findByText(/Instant payments settle through/);
    await expectNoAxeViolations();
    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    await screen.findByRole("alertdialog", { name: "Replace “Instant payments”" });
    await expectNoAxeViolations();
    await userEvent.keyboard("{Escape}");
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    await screen.findByRole("alertdialog", { name: "Delete “Instant payments”?" });
    await expectNoAxeViolations();
  });
});
