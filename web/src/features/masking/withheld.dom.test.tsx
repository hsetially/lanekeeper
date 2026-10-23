import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { serveScenario } from "@/mocks/design/file/serve";
import { renderApp } from "@/test/app";

// A file the server denies (a keystore, a certificate, a key: D79, D104) is known by name, size and hash only. This is the File page
// for one, served by the design scenario of the handoff's frame 1n, and what it must not do: no edit, upload, delete, revert, PR or
// download; no text, no code box, no picture; no diff; and no request for content that does not exist (a comparison, a blob, the file
// from Git or the baseline). The screen is a statement and a few facts.

configure({ asyncUtilTimeout: 5000 });
vi.setConfig({ testTimeout: 60_000 });

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const PATH = "security/keystore.jks";
const url = `/swimlanes/sitb/files?path=${encodeURIComponent(PATH)}`;
const WRITES = ["Edit", "Upload", "Delete", "Raise PR", "Download", "Revert", "Enable editing", "Compare with Git"];

/** What a request for content is: a comparison, a blob, or a read from anywhere but NFS. */
const asksForContent = (path: string): boolean => path.startsWith("/api/v1/compare") || path.startsWith("/api/v1/blobs/") || /[?&]source=(git|baseline|blob)\b/.test(path);

async function openDenied() {
  const app = renderApp(url, serveScenario("design-file-denied"));
  await screen.findByRole("heading", { level: 1, name: "keystore.jks" });
  await screen.findByText("Content withheld: sensitive file type");
  return app;
}

describe("the File page of a denied file", () => {
  it("says the content is withheld, and shows the name, the size and the hash", async () => {
    await openDenied();
    const text = document.body.textContent;
    expect(text).toContain("keystore.jks");
    expect(text).toContain("3.2 KB");
    expect(text).toContain("9d2c47e");
  });

  it("offers no write, no download and no way to start one", async () => {
    await openDenied();
    for (const name of WRITES) expect(screen.queryByRole("button", { name }), name).toBeNull();
    expect(screen.queryByRole("link", { name: /download/i })).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("has no copy button for the hash of a file whose content is withheld, in the head or in the card (the path and the link to the file may be copied)", async () => {
    await openDenied();
    expect(screen.queryByRole("button", { name: "Copy hash" })).toBeNull();
    expect(screen.queryAllByRole("button", { name: /^Copy .*hash/i })).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Copy path" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy link to this file" })).toBeInTheDocument();
  });

  it("has no copy button for the hash of a version in the History panel either", async () => {
    renderApp(`${url}&panel=history`, serveScenario("design-file-denied"));
    await screen.findByText("Content withheld: sensitive file type");
    const panel = await screen.findByRole("complementary", { name: "History" });
    await within(panel).findAllByRole("listitem");
    expect(within(panel).queryByRole("button", { name: "Copy hash" })).toBeNull();
  });

  it("shows no text, no code, no picture and no diff", async () => {
    await openDenied();
    for (const role of ["table", "img", "grid", "row"]) expect(screen.queryAllByRole(role), role).toHaveLength(0);
    expect(document.querySelector("pre, code, [data-slot='virtual-list']")).toBeNull();
  });

  it("asks for no content that does not exist, on any tab, and shows the same statement on each", async () => {
    const app = await openDenied();
    const user = userEvent.setup({ delay: null });
    for (const name of [/^Git/, /^Baseline/, /^Diff/, /^NFS/]) {
      await user.click(await screen.findByRole("tab", { name }));
      await waitFor(() => expect(screen.getByText("Content withheld: sensitive file type")).toBeInTheDocument());
      expect(screen.queryAllByRole("table")).toHaveLength(0);
    }
    expect(app.calls.filter((c) => asksForContent(c.path)).map((c) => c.path)).toEqual([]);
  });

  it("does not start an edit from the keyboard", async () => {
    await openDenied();
    const user = userEvent.setup({ delay: null });
    await user.keyboard("e");
    expect(screen.queryByText(/Editing NFS/)).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});
