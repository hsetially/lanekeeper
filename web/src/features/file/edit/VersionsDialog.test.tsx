import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { HASH_B, problem, renderFile, routes } from "../test-utils";
import { VersionsDialog } from "./VersionsDialog";

// After a 409 the person needs to see what is on NFS now. The contract has no comparison of two stored versions, so the dialog
// shows the two texts side by side and says so: it computes no difference (rule 7). The version on NFS now is read by its hash.

const CONTENT = "GET /api/v1/swimlanes/sitb/files/content";
const content = (text: string) => ({ path: "a.yml", hash: HASH_B, class: "structured", format: { eol: "lf", encoding: "utf8", bom: false }, size: text.length, content_withheld: false, text });
const props = { open: true, onOpenChange: vi.fn(), lane: "sitb", path: "a.yml", name: "a.yml", currentHash: HASH_B, yours: "a: mine\nb: 2\n" };

describe("VersionsDialog", () => {
  it("shows the version on NFS now beside the person's own, and says no difference is computed", async () => {
    renderFile(<VersionsDialog {...props} />, { api: routes({ [CONTENT]: () => content("a: theirs\nb: 2\n") }) });
    const dialog = await screen.findByRole("dialog", { name: "Compare versions" });
    expect(within(dialog).getByText(/does not merge two versions/)).toBeInTheDocument();
    expect(await within(dialog).findByRole("table", { name: "a.yml (on NFS now)" })).toBeInTheDocument();
    expect(within(dialog).getByRole("table", { name: "a.yml (yours)" })).toBeInTheDocument();
    expect(within(dialog).getByText("a: theirs")).toBeInTheDocument();
    expect(within(dialog).getByText("a: mine")).toBeInTheDocument();
  });

  it("reads the version on NFS now by its hash, which names the same bytes forever", async () => {
    const { calls } = renderFile(<VersionsDialog {...props} />, { api: routes({ [CONTENT]: () => content("x") }) });
    await screen.findByRole("table", { name: "a.yml (on NFS now)" });
    const query = new URL(calls.find((c) => c.path.includes("/files/content"))?.path ?? "", "http://x").searchParams;
    expect(Object.fromEntries(query)).toEqual({ path: "a.yml", source: "blob", hash: HASH_B });
  });

  it("says when the version cannot be read, with the request id, and still shows the person's own", async () => {
    renderFile(<VersionsDialog {...props} />, { api: routes({ [CONTENT]: () => problem(403, "forbidden") }) });
    expect(await screen.findByText("Couldn't load the version on NFS now")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "a.yml (yours)" })).toBeInTheDocument();
  });

  it("closes", async () => {
    const onOpenChange = vi.fn();
    renderFile(<VersionsDialog {...props} onOpenChange={onOpenChange} />, { api: routes({ [CONTENT]: () => content("x") }) });
    await userEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
