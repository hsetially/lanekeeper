import { act, configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { serveScenario } from "@/mocks/design/file/serve";
import { json, problem, renderApp, type Call } from "@/test/app";
import type { fakeMonaco } from "./fake-monaco";

// Editing, saving and the rest of the writes, from the file screen: the page's part in them. The editor is a stand-in (Monaco does
// not run in jsdom); the dialogs and the editor have their own tests. What is asserted here is what the page decides: when editing
// starts and ends, which hash a save names, and what is shown for each answer the server can give (W17: the answer is the server's,
// never assumed).

const fake = vi.hoisted((): { current: unknown } => ({ current: undefined }));
vi.mock("@/editor/loader", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/editor/loader")>();
  const { fakeMonaco: make } = await import("./fake-monaco");
  const m = make();
  fake.current = m;
  return { ...real, loadEditor: () => Promise.resolve(m.api) };
});
const editor = () => fake.current as ReturnType<typeof fakeMonaco>;

configure({ asyncUtilTimeout: 5000 });

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const url = (path = PATH, extra = "") => `/swimlanes/sitb/files?path=${encodeURIComponent(path)}${extra}`;
const open = (scenario = "design-file-edit", override?: Parameters<typeof serveScenario>[1], to = url()) => renderApp(to, serveScenario(scenario, override));
const typeInEditor = (text: string): void => {
  act(() => editor().model.setValue(text));
};

async function startEditing(scenario = "design-file-edit", override?: Parameters<typeof serveScenario>[1]) {
  const view = open(scenario, override);
  await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
  await waitFor(() => expect(editor().monaco.editor.create).toHaveBeenCalled());
  return view;
}

const puts = (calls: Call[]) => calls.filter((c) => c.method === "PUT" && c.path === "/api/v1/swimlanes/sitb/files");

describe("starting and leaving the editor", () => {
  it("opens the editor on the file's text with the Edit button, and says that the file is being edited", async () => {
    await startEditing();
    expect(await screen.findByText("Editing NFS")).toBeInTheDocument();
    expect(screen.getByText("Editing", { selector: "[data-slot=badge]" })).toBeInTheDocument();
    const text = editor().model.getValue();
    expect(text.split("\r\n")).toHaveLength(413);
    expect(text).toContain("clientSecret: ${TELLER_CLIENT_SECRET}");
    const save = screen.getByRole("button", { name: "Save to sitb NFS" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    expect(save).toHaveAccessibleDescription("No changes to save");
  });

  it("opens the editor with the e key", async () => {
    open();
    await screen.findByRole("button", { name: "Edit" });
    await userEvent.keyboard("e");
    await waitFor(() => expect(editor().monaco.editor.create).toHaveBeenCalled());
    expect(await screen.findByText("Editing NFS")).toBeInTheDocument();
  });

  it("does not open the editor for a Viewer, with the key or the button", async () => {
    open("design-file-viewer", undefined, url());
    const edit = await screen.findByRole("button", { name: "Edit" });
    await userEvent.click(edit);
    await userEvent.keyboard("e");
    expect(screen.queryByText("Editing NFS")).not.toBeInTheDocument();
  });

  it("does not open the editor for a file with a masked value", async () => {
    open("design-file-core");
    await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await userEvent.keyboard("e");
    expect(screen.queryByText("Editing NFS")).not.toBeInTheDocument();
  });

  it("leaves at once with Escape when nothing was changed", async () => {
    await startEditing();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByText("Editing NFS")).not.toBeInTheDocument());
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("asks before throwing away changes, keeps them on Keep editing, and drops them on Discard", async () => {
    await startEditing();
    typeInEditor("changed: true\r\n");
    expect(await screen.findByText("Unsaved changes")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Discard your changes?" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Keep editing" }));
    expect(screen.getByText("Editing NFS")).toBeInTheDocument();
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await userEvent.click(within(await screen.findByRole("alertdialog", { name: "Discard your changes?" })).getByRole("button", { name: "Discard changes" }));
    await waitFor(() => expect(screen.queryByText("Editing NFS")).not.toBeInTheDocument());
  });

  it("opens a large file read-only and starts the editor when asked", async () => {
    open("design-file-large", undefined, url("security/security-roles.yml"));
    await userEvent.click(await screen.findByRole("button", { name: "Enable editing" }));
    await waitFor(() => expect(editor().monaco.editor.create).toHaveBeenCalled());
    expect(await screen.findByText("Editing NFS")).toBeInTheDocument();
    expect(screen.queryByText("Large file · 6,140 lines")).not.toBeInTheDocument();
  });
});

describe("the file moving on while it is open", () => {
  it("says the file changed on NFS while it is being edited, before the save is refused, and keeps the person's text", async () => {
    const original = serveScenario("design-file-edit");
    let reads = 0;
    const { queryClient } = await startEditing("design-file-edit", (call) => {
      if (call.method !== "GET" || !call.path.includes("/files/content") || !call.path.includes("source=nfs")) return undefined;
      reads += 1;
      if (reads === 1) return undefined;
      // The same file, with a newer hash: someone else saved it.
      return Promise.resolve(original(call)).then(async (response) => json({ ...((await response.json()) as Record<string, unknown>), hash: "c19a4e2".repeat(10).slice(0, 64) }));
    });
    typeInEditor("changed: true\r\n");
    expect(screen.queryByText("This file changed on NFS while you were editing")).not.toBeInTheDocument();
    await act(async () => {
      await queryClient.refetchQueries({ predicate: (query) => query.queryKey[0] === "file-head" });
    });
    expect(await screen.findByText("This file changed on NFS while you were editing")).toBeInTheDocument();
    expect(editor().model.getValue()).toBe("changed: true\r\n");
  });
});

describe("saving", () => {
  it("asks first, never saves silently, and names the hash the edit is based on", async () => {
    const { calls } = await startEditing();
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Save tx-infinity-core-sit7.yml to sitb?" });
    expect(within(dialog).getByText("Saving writes straight to the sitb NFS share. Git changes only through a PR.")).toBeInTheDocument();
    expect(within(dialog).getByText("a3f9c21")).toBeInTheDocument();
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus());
    expect(puts(calls)).toHaveLength(0);
  });

  it("opens the review with the save key when there are changes, and not when there are none", async () => {
    await startEditing();
    await userEvent.keyboard("{Control>}s{/Control}");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    typeInEditor("changed: true\r\n");
    await screen.findByText("Unsaved changes");
    await userEvent.keyboard("{Control>}s{/Control}");
    expect(await screen.findByRole("alertdialog", { name: /^Save / })).toBeInTheDocument();
  });

  it("writes with the expected hash, the text and an idempotency key, and says it is saved only once the server did", async () => {
    const { calls } = await startEditing();
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save to sitb NFS" }));
    expect(await screen.findByText("Saved to sitb NFS")).toBeInTheDocument();
    const put = puts(calls)[0];
    expect(JSON.parse(put?.body ?? "{}")).toEqual({ path: PATH, expected: { state: "hash", hash: "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a" }, content: "changed: true\r\n" });
    expect(put?.headers.get("Idempotency-Key")).toBeTruthy();
    // The editor is left, and the file is shown again.
    await waitFor(() => expect(screen.queryByText("Editing NFS")).not.toBeInTheDocument());
    expect(screen.getByText(/The file on sitb NFS is the new version\./)).toBeInTheDocument();
  });

  it("says nothing is saved while the request is still out", async () => {
    await startEditing("design-file-edit", (call) => (call.method === "PUT" ? new Promise<Response>(() => undefined) : undefined));
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    expect(await screen.findByRole("button", { name: "Saving…" })).toBeInTheDocument();
    expect(screen.queryByText("Saved to sitb NFS")).not.toBeInTheDocument();
    expect(screen.getByText("Editing NFS")).toBeInTheDocument();
  });
});

describe("what the server can answer to a save", () => {
  it("409: says someone else changed the file, writes nothing, and keeps the person's text", async () => {
    await startEditing("design-file-edit-conflict");
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    const conflict = await screen.findByRole("dialog", { name: "File changed on NFS while you were editing" });
    expect(within(conflict).getByText("Your save was not applied. Nothing on NFS changed.")).toBeInTheDocument();
    expect(within(conflict).getByText("a3f9c21")).toBeInTheDocument();
    expect(within(conflict).getByText("c19a4e2")).toBeInTheDocument();
    expect(within(conflict).getByText("req-mock-0001")).toBeInTheDocument();
    expect(screen.queryByText("Saved to sitb NFS")).not.toBeInTheDocument();
    expect(editor().model.getValue()).toBe("changed: true\r\n");
  });

  it("409, Reload and re-apply: keeps the text, and the next save names the version that is on NFS now", async () => {
    let seen = 0;
    const { calls } = await startEditing("design-file-edit-conflict", (call) => {
      if (call.method !== "PUT") return undefined;
      seen += 1;
      return seen === 1 ? undefined : new Response(JSON.stringify({ outcome: "applied", hash: "d".repeat(64), audit_id: 1 }), { status: 200, headers: { "content-type": "application/json" } });
    });
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(await screen.findByRole("button", { name: "Reload and re-apply" }));
    expect(await screen.findByText(/Based on the version that is on NFS now\. Saving replaces it with yours\./)).toBeInTheDocument();
    expect(editor().model.getValue()).toBe("changed: true\r\n");
    await userEvent.click(screen.getByRole("button", { name: "Save to sitb NFS" }));
    const review = await screen.findByRole("alertdialog");
    expect(within(review).getByText(/replaces the version that is on NFS now/)).toBeInTheDocument();
    await userEvent.click(within(review).getByRole("button", { name: "Save to sitb NFS" }));
    await screen.findByText("Saved to sitb NFS");
    expect(JSON.parse(puts(calls)[1]?.body ?? "{}")).toMatchObject({ expected: { state: "hash", hash: "c19a4e2c19a4e2c19a4e2c19a4e2c19a4e2c19a4e2c19a4e2c19a4e2c19a4e2c" } });
  });

  it("409 because the file was removed: says so, and Save as a new file makes the next save expect no file", async () => {
    let seen = 0;
    const { calls } = await startEditing("design-file-edit-conflict", (call) => {
      if (call.method !== "PUT") return undefined;
      seen += 1;
      return seen === 1 ? problem(409, "conflict", { current: null }) : json({ outcome: "applied", hash: "d".repeat(64), audit_id: 1 });
    });
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    const conflict = await screen.findByRole("dialog", { name: "File changed on NFS while you were editing" });
    expect(within(conflict).getByText("Your save was not applied. The file was removed from NFS while you were editing.")).toBeInTheDocument();
    await userEvent.click(within(conflict).getByRole("button", { name: "Save as a new file" }));
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    const review = await screen.findByRole("alertdialog");
    expect(within(review).getByText("No file: a new one is created")).toBeInTheDocument();
    await userEvent.click(within(review).getByRole("button", { name: "Save to sitb NFS" }));
    await screen.findByText("Saved to sitb NFS");
    expect(JSON.parse(puts(calls)[1]?.body ?? "{}")).toMatchObject({ expected: { state: "absent" } });
  });

  it("409, Discard my changes: leaves the editor", async () => {
    await startEditing("design-file-edit-conflict");
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(await screen.findByRole("button", { name: "Discard my changes" }));
    await waitFor(() => expect(screen.queryByText("Editing NFS")).not.toBeInTheDocument());
  });

  it("409, View current: shows what is on NFS now beside the person's text, and computes no diff", async () => {
    const { calls } = await startEditing("design-file-edit-conflict");
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(await screen.findByRole("button", { name: "View current" }));
    const versions = await screen.findByRole("dialog", { name: "Compare versions" });
    expect(within(versions).getByRole("region", { name: "On NFS now" })).toBeInTheDocument();
    expect(within(versions).getByRole("region", { name: "Yours" })).toBeInTheDocument();
    await waitFor(() => expect(calls.some((c) => c.path.includes("source=blob") && c.path.includes("c19a4e2"))).toBe(true));
  });

  it("423: says the swimlane is locked until it is adopted, and why, and keeps the text", async () => {
    await startEditing("design-file-edit-locked");
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    const locked = await screen.findByRole("dialog", { name: "sitb is locked until it is adopted" });
    expect(await within(locked).findByText("412")).toBeInTheDocument();
    expect(within(locked).getByRole("link", { name: "Start adoption" })).toHaveAttribute("href", "/swimlanes/sitb/adopt");
    expect(editor().model.getValue()).toBe("changed: true\r\n");
    expect(screen.queryByText("Saved to sitb NFS")).not.toBeInTheDocument();
  });

  it("a person whose writes need approval submits for approval, and is told it is not applied yet", async () => {
    await startEditing("design-file-edit-approval");
    typeInEditor("changed: true\r\n");
    expect(await screen.findByText("Needs approval from an Admin")).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "Submit for approval" }));
    const review = await screen.findByRole("alertdialog");
    expect(within(review).getByText(/Nothing changes on sitb until then\./)).toBeInTheDocument();
    await userEvent.click(within(review).getByRole("button", { name: "Submit for approval" }));
    const sent = await screen.findByRole("dialog", { name: "Sent for approval" });
    expect(within(sent).getByText("AP-9")).toBeInTheDocument();
    expect(within(sent).getByText(/Nothing has changed on NFS yet/)).toBeInTheDocument();
    expect(within(sent).getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/9");
    expect(screen.queryByText("Saved to sitb NFS")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("Editing NFS")).not.toBeInTheDocument());
  });

  it("422 with a secret: marks the line in the editor, lists the problem, blocks Save, and clears as soon as the person types", async () => {
    await startEditing("design-file-edit-secret");
    typeInEditor("privateKey: x\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    const problems = await screen.findByRole("region", { name: "Problems" });
    expect(within(problems).getByText(/Line 120 looks like a secret \(private_key\)/)).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("Save blocked: line 120 looks like a secret (private_key)");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(editor().model.getValue()).toBe("privateKey: x\r\n");
    typeInEditor("privateKey: y\r\n");
    await waitFor(() => expect(screen.queryByRole("region", { name: "Problems" })).not.toBeInTheDocument());
  });

  it("a refused save that is not the content stays in the review, with the request id", async () => {
    await startEditing("design-file-edit", (call) => (call.method === "PUT" ? problem(403, "forbidden") : undefined));
    typeInEditor("changed: true\r\n");
    await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
    const alert = await within(await screen.findByRole("alertdialog")).findByRole("alert");
    expect(alert).toHaveTextContent("Nothing was written");
    expect(alert).toHaveTextContent("req-test-1");
    expect(screen.getByText("Editing NFS")).toBeInTheDocument();
  });
});

describe("the other writes of the screen", () => {
  it("converts the line endings from the Diff tab, with the file's text and the hash it has", async () => {
    const { calls } = open("design-file-lineending", undefined, url("tx-infinity-api/tx-infinity-gateway.yml", "&tab=diff"));
    await userEvent.click(await screen.findByRole("button", { name: "Convert NFS file to LF" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Convert tx-infinity-gateway.yml to LF?" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Convert to LF" }));
    expect(await screen.findByText("Line endings converted")).toBeInTheDocument();
    const body = JSON.parse(puts(calls)[0]?.body ?? "{}") as { content: string; change_eol: string; expected: unknown };
    expect(body.change_eol).toBe("lf");
    expect(body.expected).toEqual({ state: "hash", hash: "d41e07b".repeat(10).slice(0, 64) });
    expect(body.content.split("\r\n").length).toBeGreaterThan(200);
  });

  it("deletes after the name is typed, with the hash and the preview, and goes back to the swimlane", async () => {
    const { calls, router } = open("design-file-edit");
    await userEvent.click(await screen.findByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    await within(dialog).findByText(/Every service reads this file\./);
    await userEvent.type(within(dialog).getByRole("textbox"), "tx-infinity-core-sit7.yml");
    await userEvent.click(within(dialog).getByRole("button", { name: /^Delete/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/swimlanes/sitb"));
    const del = calls.find((c) => c.method === "DELETE");
    const query = new URL(del?.path ?? "", "http://x").searchParams;
    expect(query.get("path")).toBe(PATH);
    expect(query.get("preview_id")).toBe("pv-mock-0001");
    expect(query.get("expected")).toBe("a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a");
  });

  it("says the file changed first when a delete is refused with 409, and offers only Close", async () => {
    open("design-file-edit-conflict");
    await userEvent.click(await screen.findByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    await within(dialog).findByText(/Every service reads this file\./);
    await userEvent.type(within(dialog).getByRole("textbox"), "tx-infinity-core-sit7.yml");
    await userEvent.click(within(dialog).getByRole("button", { name: /^Delete/ }));
    const conflict = await screen.findByRole("dialog", { name: "The file changed on NFS first" });
    expect(within(conflict).getByText(/Your delete was not applied/)).toBeInTheDocument();
    expect(within(conflict).queryByRole("button", { name: "Reload and re-apply" })).not.toBeInTheDocument();
  });
});
