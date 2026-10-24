import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderFile, routes } from "../test-utils";
import { RefPicker } from "./RefPicker";

// The ref picker of the Diff tab (1a, the open menu): the sources of the left side — the file in Git and the baseline —
// then another swimlane, then tags and branches. Choosing one only says which; the screen puts it in the URL.

const lane = (id: string) => ({ id, display_name: id, tenants: ["sit1"], agent: "connected", last_scan_at: null, base_version: null, drift_counts: [], finding_count: 0 });
const ref = (name: string, kind: "tag" | "branch") => ({ name, kind, commit: "a".repeat(40), branch_kind: kind === "branch" ? "deployable" : null });

const api = routes({
  "GET /api/v1/swimlanes": () => ({ items: [lane("sit"), lane("sitb"), lane("sitc"), lane("presita")], next: null }),
  "GET /api/v1/git/refs": (call) => {
    const query = new URL(call.path, "http://x").searchParams;
    return query.get("kind") === "tag" ? { items: [ref("v2.3.1", "tag"), ref("v2.3.0", "tag")], next: null } : { items: [ref("sit7", "branch"), ref("sit9", "branch")], next: null };
  },
});

const props = { label: "Left" as const, text: "Git (sit7 head)", lane: "sitb", tenant: "sit7", gitLabel: "Git (sit7 head)", onSelect: vi.fn() };

async function open() {
  const view = renderFile(<RefPicker {...props} />, { api });
  await userEvent.click(await screen.findByRole("button", { name: /Left/ }));
  return view;
}

describe("RefPicker", () => {
  it("shows what the side is now, and that it opens a list", async () => {
    renderFile(<RefPicker {...props} />, { api });
    const button = await screen.findByRole("button", { name: /Left/ });
    expect(button).toHaveTextContent("Git (sit7 head)");
    expect(button).toHaveAttribute("aria-haspopup", "listbox");
    expect(button).toHaveAttribute("aria-expanded", "false");
  });

  it("lists the sources, the other swimlanes without this one, the tags and the branches", async () => {
    await open();
    const list = await screen.findByRole("listbox");
    const names = async (group: string) => within(await within(list).findByRole("group", { name: group })).getAllByRole("option").map((o) => o.textContent);
    expect(await names("Sources")).toEqual(["Git (sit7 head)", "Baseline"]);
    expect(await names("Another swimlane")).toEqual(["sit", "sitc", "presita"]);
    expect(await names("Tags")).toEqual(["v2.3.1", "v2.3.0"]);
    expect(await names("Branches")).toEqual(["sit7", "sit9"]);
  });

  it("asks for tags of the base repository and branches of the tenant repository for a tenant file", async () => {
    const { calls } = await open();
    await screen.findByRole("listbox");
    await within(await screen.findByRole("listbox")).findByRole("group", { name: "Branches" });
    const refs = calls.filter((c) => c.path.startsWith("/api/v1/git/refs")).map((c) => new URL(c.path, "http://x").searchParams);
    expect(refs.map((q) => `${q.get("repo")}:${q.get("kind")}`).sort()).toEqual(["base:tag", "tenant:branch"]);
  });

  it("reads branches from the base repository for a base file", async () => {
    const { calls } = renderFile(<RefPicker {...props} tenant={null} gitLabel="Git (v2.3.1 + 4)" />, { api });
    await userEvent.click(await screen.findByRole("button", { name: /Left/ }));
    await within(await screen.findByRole("listbox")).findByRole("group", { name: "Branches" });
    const refs = calls.filter((c) => c.path.startsWith("/api/v1/git/refs")).map((c) => new URL(c.path, "http://x").searchParams);
    expect(refs.map((q) => `${q.get("repo")}:${q.get("kind")}`).sort()).toEqual(["base:branch", "base:tag"]);
  });

  it("does not ask for tags or branches until the list is opened", async () => {
    const view = renderFile(<RefPicker {...props} />, { api });
    await screen.findByRole("button", { name: /Left/ });
    expect(view.calls.filter((c) => c.path.startsWith("/api/v1/git/refs"))).toHaveLength(0);
  });

  it.each([
    ["Baseline", { kind: "baseline", lane: "sitb" }],
    ["sitc", { kind: "lane", id: "sitc" }],
    ["v2.3.0", { kind: "tag", name: "v2.3.0" }],
    ["sit9", { kind: "branch", name: "sit9" }],
    ["Git (sit7 head)", { kind: "git" }],
  ])("says %s was chosen, and closes", async (name, side) => {
    const onSelect = vi.fn();
    renderFile(<RefPicker {...props} onSelect={onSelect} />, { api });
    await userEvent.click(await screen.findByRole("button", { name: /Left/ }));
    await userEvent.click(await screen.findByRole("option", { name }));
    expect(onSelect).toHaveBeenCalledWith(side);
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("filters the list as the person types", async () => {
    await open();
    await userEvent.type(await screen.findByPlaceholderText("Filter…"), "v2.3.0");
    expect(await screen.findByRole("option", { name: "v2.3.0" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "sitc" })).not.toBeInTheDocument();
  });

  it("says so when tags and branches cannot be read, and still offers the rest", async () => {
    renderFile(<RefPicker {...props} />, { api: (call) => (call.path.startsWith("/api/v1/git/refs") ? new Response(JSON.stringify({ type: "urn:lanekeeper:problem:forbidden", title: "Forbidden", status: 403 }), { status: 403, headers: { "content-type": "application/problem+json" } }) : (api(call))) });
    await userEvent.click(await screen.findByRole("button", { name: /Left/ }));
    expect(await screen.findByText("Tags and branches could not be read.")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Baseline" })).toBeInTheDocument();
  });

  it("draws a ref name as data", async () => {
    renderFile(<RefPicker {...props} />, { api: routes({ "GET /api/v1/swimlanes": () => ({ items: [lane("sitb")], next: null }), "GET /api/v1/git/refs": () => ({ items: [ref("v1‮0", "tag")], next: null }) }) });
    await userEvent.click(await screen.findByRole("button", { name: /Left/ }));
    expect((await screen.findAllByText("<U+202E>")).length).toBeGreaterThan(0);
  });

  it("is not a list at all for a side that is fixed", async () => {
    renderFile(<RefPicker {...props} label="Right" text="NFS (current)" fixed />, { api });
    expect(await screen.findByText("NFS (current)")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Right/ })).not.toBeInTheDocument();
  });
});
