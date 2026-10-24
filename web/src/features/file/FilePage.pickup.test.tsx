import { act, configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { serveScenario } from "@/mocks/design/file/serve";
import { renderApp } from "@/test/app";
import type { fakeMonaco } from "./fake-monaco";

// What the "Saved" banner of the file page offers once a write was applied (T6): by the file's pickup state, the buttons that let a
// person tell the services (D85). The editor is a stand-in (Monaco does not run in jsdom); the dialogs have their own tests in
// src/features/notify. What is asserted here is the page's part: which buttons the banner holds, and for which state of the file.

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
const url = `/swimlanes/sitb/files?path=${encodeURIComponent(PATH)}`;

/** Edit the file and save it: the hub's answer is "applied", and the page shows the banner. */
async function saveFile(scenario: string, role?: Parameters<typeof renderApp>[2]) {
  const app = renderApp(url, serveScenario(scenario), role);
  await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
  await waitFor(() => expect(editor().monaco.editor.create).toHaveBeenCalled());
  act(() => editor().model.setValue("changed: true\r\n"));
  await userEvent.click(await screen.findByRole("button", { name: "Save to sitb NFS" }));
  await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save to sitb NFS" }));
  await screen.findByText("Saved to sitb NFS");
  return app;
}
const banner = (): HTMLElement => {
  const found = screen.getByText("Saved to sitb NFS").closest<HTMLElement>('[data-slot="alert"]');
  if (found === null) throw new Error("the Saved banner is not an Alert");
  return found;
};

describe("the Saved banner of a file that services have not picked up", () => {
  it("says services pick it up after a notify or a restart, and offers to notify them and to restart them", async () => {
    await saveFile("design-notify-file-saved");
    expect(within(banner()).getByText(/Services pick it up after a notify or a restart\./)).toBeInTheDocument();
    expect(await within(banner()).findByRole("button", { name: "Notify services" })).toBeEnabled();
    expect(within(banner()).getByRole("button", { name: "Restart services" })).toBeEnabled();
    expect(within(banner()).queryByRole("button", { name: "Restart config-server (type its name to confirm)" })).toBeNull();
  });

  it("offers the config-server's restart, and nothing else, for a file the config-server serves only after it restarts", async () => {
    await saveFile("design-notify-file-config-server");
    expect(await within(banner()).findByRole("button", { name: "Restart config-server (type its name to confirm)" })).toBeEnabled();
    expect(within(banner()).queryByRole("button", { name: "Notify services" })).toBeNull();
    expect(within(banner()).queryByRole("button", { name: "Restart services" })).toBeNull();
  });

  it("offers nothing for a file that no service waits on: there is nothing for a person to do", async () => {
    await saveFile("design-notify-file-live");
    expect(screen.getByText(/The file on sitb NFS is the new version\./)).toBeInTheDocument();
    expect(within(banner()).queryByText(/Services pick it up/)).toBeNull();
    expect(within(banner()).queryByRole("button", { name: /Notify services|Restart/ })).toBeNull();
  });

  it("opens the notify dialog from the banner, which names this file and sends nothing until it is confirmed", async () => {
    const app = await saveFile("design-notify-file-saved");
    await userEvent.click(await within(banner()).findByRole("button", { name: "Notify services" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Notify services in sitb?" });
    expect(within(dialog).getAllByText(PATH).length).toBeGreaterThan(0);
    expect(app.calls.filter((c) => c.method === "POST" && c.path.endsWith("/notify"))).toHaveLength(0);
  });
});
