import { configure, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { serveScenario } from "@/mocks/design/file/serve";
import { meBody } from "@/mocks/me";
import { json, problem, renderApp } from "@/test/app";

// The file view as the design draws it, served by the design scenarios (src/mocks/design/file): the head, the tabs, the panels
// and the states of the States index for a file (Viewer, agent offline, withheld, binary, large, a missing file). The writes
// have their own tests next to the edit surface and the dialogs; the page's part in them is in FilePage.edit.test.tsx.

// The first screen of a run loads two lazy chunks, and the machine may be busy: waiting is cheap, a flaky test is not.
configure({ asyncUtilTimeout: 5000 });

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const files = (path = PATH, extra = "") => `/swimlanes/sitb/files?path=${encodeURIComponent(path)}${extra}`;
const open = (scenario = "design-file-core", url = files(), override?: Parameters<typeof serveScenario>[1]) => renderApp(url, serveScenario(scenario, override));

describe("the head", () => {
  it("has the logical file as its one title, with the path, the facts, the hash and the drift state", async () => {
    open();
    expect(await screen.findByRole("heading", { level: 1, name: "tx-infinity-core.yml" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    const head = screen.getByRole("region", { name: "File" });
    expect(within(head).getByText("NFS ahead")).toBeInTheDocument();
    expect(await within(head).findByText("412 lines")).toBeInTheDocument();
    expect(within(head).getByText("CRLF")).toBeInTheDocument();
    expect(within(head).getByText("UTF-8")).toBeInTheDocument();
    expect(within(head).getByText("a3f9c21")).toBeInTheDocument();
    expect(within(head).getByText(PATH)).toBeInTheDocument();
  });

  it("marks a tenant file as the effective file for its base file", async () => {
    open();
    expect(await screen.findByText("Effective file for tx-infinity-core")).toBeInTheDocument();
  });

  it("shows the file's name and holds the place of the facts before the data is there", async () => {
    renderApp(files(), (call) => (call.path.startsWith("/api/v1/swimlanes/sitb/files/content") ? new Promise<Response>(() => undefined) : serveScenario("design-file-core")(call)));
    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent("tx-infinity-core-sit7.yml");
    expect(await screen.findByLabelText("Loading file")).toHaveAttribute("aria-busy", "true");
  });

  it("offers the actions an Admin has, and History and Docs for this file", async () => {
    open();
    const head = await screen.findByRole("region", { name: "File" });
    for (const name of ["Raise PR", "Upload", "Delete", "History", "Docs for this file"]) expect(await within(head).findByRole("button", { name })).toBeInTheDocument();
    expect(within(head).getByRole("button", { name: "Edit" })).toBeInTheDocument();
  });
});

describe("the content", () => {
  it("shows the file's lines, with the server's marker where a value was masked, and keeps Edit off for it", async () => {
    open();
    const code = await screen.findByRole("table", { name: "tx-infinity-core-sit7.yml" });
    expect(within(code).getByText("tenant: sit7")).toBeInTheDocument();
    expect(await screen.findByText("Edit")).toBeInTheDocument();
    const edit = screen.getByRole("button", { name: "Edit" });
    expect(edit).toHaveAttribute("aria-disabled", "true");
    expect(edit).toHaveAccessibleDescription("This view has masked values, so it is not the file and cannot be edited here.");
  });

  it("reads the file from Git on the Git tab, from the tenant's branch", async () => {
    const { calls } = open();
    await userEvent.click(await screen.findByRole("tab", { name: /^Git/ }));
    await waitFor(() => expect(calls.some((c) => c.path.includes("source=git"))).toBe(true));
    const query = new URL(calls.find((c) => c.path.includes("source=git"))?.path ?? "", "http://x").searchParams;
    expect(Object.fromEntries(query)).toEqual({ path: PATH, source: "git", repo: "tenant", ref: "sit7" });
    expect(await screen.findByText("LF")).toBeInTheDocument();
  });

  it("reads the file from the baseline on the Baseline tab", async () => {
    const { calls } = open();
    await userEvent.click(await screen.findByRole("tab", { name: /^Baseline/ }));
    await waitFor(() => expect(calls.some((c) => c.path.includes("source=baseline"))).toBe(true));
    expect(await screen.findByText("410 lines")).toBeInTheDocument();
  });

  it("says the file is not in Git, rather than failing, when Git does not have it", async () => {
    open("design-file-core", files(PATH, "&tab=git"), (call) => (call.path.includes("source=git") ? problem(404, "not_found") : undefined));
    expect(await screen.findByText("tx-infinity-core-sit7.yml is not in Git")).toBeInTheDocument();
  });

  it("puts the tab in the address, so that the back button goes back to the tab before", async () => {
    const { router } = open();
    await userEvent.click(await screen.findByRole("tab", { name: /^Baseline/ }));
    await waitFor(() => expect(router.state.location.search).toMatchObject({ tab: "baseline" }));
    router.history.back();
    await waitFor(() => expect(screen.getByRole("tab", { name: /^NFS/ })).toHaveAttribute("aria-selected", "true"));
  });

  it("takes only a tab it knows from the address", async () => {
    open("design-file-core", files(PATH, "&tab=%3Cscript%3E"));
    expect(await screen.findByRole("tab", { name: /^NFS/ })).toHaveAttribute("aria-selected", "true");
  });

  it("shows the changes the engine found on the Diff tab, and how many", async () => {
    open("design-file-core", files(PATH, "&tab=diff"));
    const grid = await screen.findByRole("grid", { name: /Diff of tx-infinity-core-sit7\.yml/ });
    expect(within(grid).getByText("@@ -10,8 +10,9 @@")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /^Diff/ })).toHaveTextContent("3");
    expect(screen.getByText("2 settings changed, 1 added")).toBeInTheDocument();
  });

  it("marks the file when only the line endings differ from Git", async () => {
    open("design-file-lineending", files("tx-infinity-api/tx-infinity-gateway.yml"));
    expect(await screen.findByText("Line endings only")).toBeInTheDocument();
  });

  it("jumps to the line in the address", async () => {
    open("design-file-core", files(PATH, "&line=300"));
    const code = await screen.findByRole("table", { name: "tx-infinity-core-sit7.yml" });
    await waitFor(() => expect(code.querySelector('[data-highlight="true"]')).not.toBeNull());
    expect(code.querySelector('[data-highlight="true"]')).toHaveTextContent("300");
  });
});

describe("without a file", () => {
  it("asks for one when the address names none, and links to the swimlane", async () => {
    const { calls } = renderApp("/swimlanes/sitb/files", serveScenario("design-file-core"));
    expect(await screen.findByRole("heading", { level: 1, name: "Choose a file" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open the swimlane" })).toHaveAttribute("href", "/swimlanes/sitb");
    expect(calls.some((c) => c.path.includes("/files/content"))).toBe(false);
  });

  it("asks the server nothing for a path that cannot be a file", async () => {
    const { calls } = open("design-file-core", files("../etc/passwd"));
    expect(await screen.findByRole("heading", { level: 1, name: "That is not a file path" })).toBeInTheDocument();
    expect(calls.some((c) => c.path.includes("/files/content"))).toBe(false);
  });

  it("says the file was not found when the server has no such file", async () => {
    open("design-file-core", files(PATH), (call) => (call.path.includes("/files/content") ? problem(404, "not_found") : undefined));
    expect(await screen.findByText("File not found")).toBeInTheDocument();
    expect(screen.getByText(`There is no ${PATH} in sitb. It may have been deleted or renamed.`)).toBeInTheDocument();
  });

  it("names the problem and its request id when the file cannot be read", async () => {
    open("design-file-core", files(PATH), (call) => (call.path.includes("/files/content") ? problem(403, "forbidden") : undefined));
    expect(await screen.findByText("Couldn't load file")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
  });

  it("says where the file was being read from when the server fails, in the design's words (States R03)", async () => {
    open("design-file-core", files(PATH), (call) => (call.path.includes("/files/content") ? problem(500, "internal") : undefined));
    expect(await screen.findByText("Couldn't load file")).toBeInTheDocument();
    expect(screen.getByText("The server returned an error while reading the file from NFS. Retry, or send the request ID to the platform team.")).toBeInTheDocument();
  });
});

describe("who may do what", () => {
  it("tells a Viewer the file is read-only, and disables every write with the role that is needed", async () => {
    open("design-file-viewer");
    expect(await screen.findByText("Read-only")).toBeInTheDocument();
    for (const name of ["Edit", "Raise PR", "Upload", "Delete"]) {
      const button = await screen.findByRole("button", { name });
      expect(button).toHaveAttribute("aria-disabled", "true");
      expect(button).toHaveAccessibleDescription("Needs the Editor role. Ask an Admin to grant it.");
    }
    expect(screen.getByRole("button", { name: "History" })).not.toHaveAttribute("aria-disabled");
  });

  it("says the agent is offline, shows the last known file, and turns the writes off", async () => {
    open("design-file-agent-offline", files(), undefined);
    expect(await screen.findByText("Agent for sitb is offline")).toBeInTheDocument();
    const upload = await screen.findByRole("button", { name: "Upload" });
    expect(upload).toHaveAttribute("aria-disabled", "true");
    expect(upload).toHaveAccessibleDescription("Unavailable while the agent is offline");
    expect(await screen.findByRole("table", { name: "tx-infinity-core-sit7.yml" })).toBeInTheDocument();
  });

  it("turns Raise PR off for a person who has no GitHub token yet, and says what to do", async () => {
    renderApp(files(), serveScenario("design-file-core", (call) => (call.path === "/api/v1/me" ? json(meBody("admin", { needs_github_token: true })) : undefined)));
    const pr = await screen.findByRole("button", { name: "Raise PR" });
    await waitFor(() => expect(pr).toHaveAttribute("aria-disabled", "true"));
    expect(pr).toHaveAccessibleDescription("Connect your GitHub token first. A pull request is raised with your own token.");
  });
});

describe("files that are not text", () => {
  it("withholds the content of a denied file, with no write action, and says why", async () => {
    open("design-file-denied", files("security/keystore.jks"));
    expect(await screen.findByText("Content withheld: sensitive file type")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Upload" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "History" })).toBeInTheDocument();
    expect(await screen.findAllByText("Not available")).toHaveLength(2);
  });

  it("shows a picture from the blob store, and no editor for it", async () => {
    open("design-file-binary", files("document-service/resources/receipt-logo.jpeg"));
    const image = await screen.findByRole("img", { name: "receipt-logo.jpeg" });
    expect(image.getAttribute("src")).toMatch(/^\/api\/v1\/blobs\/[0-9a-f]{64}$/);
    const edit = screen.getByRole("button", { name: "Edit" });
    expect(edit).toHaveAccessibleDescription("A binary file cannot be edited here. Upload a new version instead.");
  });

  it("opens a large file read-only, says so, and offers to enable editing", async () => {
    open("design-file-large", files("security/security-roles.yml"));
    const banner = await screen.findByText("Large file · 6,140 lines");
    expect(banner).toBeInTheDocument();
    expect(screen.getByText("Opened read-only first so the page stays fast.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Enable editing" })).toBeInTheDocument();
  });

  it("puts what the file brings (large, withheld) under the head, as the handoff has it, and the placeholder head stays where it was", async () => {
    open("design-file-large", files("security/security-roles.yml"));
    const head = await screen.findByRole("region", { name: "File" });
    const banner = await screen.findByText("Large file · 6,140 lines");
    // The head is on the page before the file comes and is the same element after it (it does not move when the banner arrives); the banner follows it.
    expect(screen.getByRole("region", { name: "File" })).toBe(head);
    expect(head.compareDocumentPosition(banner) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe("the panels", () => {
  /** The History panel once the file has come: the body of the page is drawn again when the file arrives (it is a placeholder before), so a panel found earlier is gone. */
  async function historyWithFile(): Promise<HTMLElement> {
    await screen.findByRole("tab", { name: /^NFS/ });
    return screen.findByRole("complementary", { name: "History" });
  }

  it("opens History from the button, puts it in the address, and closes it again", async () => {
    const { router } = open();
    await userEvent.click(await screen.findByRole("button", { name: "History" }));
    const panel = await screen.findByRole("complementary", { name: "History" });
    expect(router.state.location.search).toMatchObject({ panel: "history" });
    expect(within(panel).getByText("Current")).toBeInTheDocument();
    await userEvent.click(within(panel).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("complementary", { name: "History" })).not.toBeInTheDocument());
    expect(router.state.location.search).not.toHaveProperty("panel");
  });

  it("opens the history panel from the address", async () => {
    open("design-file-core", files(PATH, "&panel=history"));
    expect(await screen.findByRole("complementary", { name: "History" })).toBeInTheDocument();
  });

  it("reverts to a version only after asking, with the hash that is on NFS now, and says so when it is done", async () => {
    const { calls } = open("design-file-core", files(PATH, "&panel=history"));
    const panel = await historyWithFile();
    const revert = (await within(panel).findAllByRole("button", { name: /^Revert to / }))[0] as HTMLElement;
    await userEvent.click(revert);
    const dialog = await screen.findByRole("alertdialog", { name: /^Revert to / });
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: /^Revert to / }));
    expect(await screen.findByText("Reverted")).toBeInTheDocument();
    const post = calls.find((c) => c.method === "POST" && c.path.endsWith("/files/revert"));
    expect(JSON.parse(post?.body ?? "{}")).toMatchObject({ path: PATH, expected: { state: "hash", hash: "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a" } });
  });

  it("tells the person the file changed first when a revert hits a conflict, and only offers Close", async () => {
    open("design-file-core", files(PATH, "&panel=history"), (call) => (call.method === "POST" && call.path.endsWith("/files/revert") ? problem(409, "conflict", { current: "c".repeat(64) }) : undefined));
    const panel = await historyWithFile();
    await userEvent.click((await within(panel).findAllByRole("button", { name: /^Revert to / }))[0] as HTMLElement);
    await userEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^Revert to / }));
    const conflict = await screen.findByRole("dialog", { name: "The file changed on NFS first" });
    expect(within(conflict).getByText(/Your revert was not applied/)).toBeInTheDocument();
    expect(within(conflict).queryByRole("button", { name: "Reload and re-apply" })).not.toBeInTheDocument();
  });

  it("opens Docs for this file with the docs and the flags", async () => {
    open("design-file-core", files(PATH, "&panel=docs"));
    await screen.findByRole("tab", { name: /^NFS/ });
    const panel = await screen.findByRole("complementary", { name: "Docs for this file" });
    expect(await within(panel).findByRole("link", { name: /Open the doc/ })).toHaveAttribute("href", "/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Fremote-itm-teller.md#doc-account-sorting");
    expect(await within(panel).findByRole("table")).toBeInTheDocument();
  });

  it("has no Docs button for a file that no document describes", async () => {
    open("design-file-lineending", files("tx-infinity-api/tx-infinity-gateway.yml"));
    await screen.findByRole("button", { name: "History" });
    await waitFor(() => expect(screen.queryByRole("button", { name: "Docs for this file" })).not.toBeInTheDocument());
  });

  it("asks for the docs of the logical file and for the flags of this swimlane", async () => {
    const { calls } = open("design-file-core", files(PATH, "&panel=docs"));
    await screen.findByRole("complementary", { name: "Docs for this file" });
    await waitFor(() => expect(calls.some((c) => c.path.startsWith("/api/v1/docs/for-file"))).toBe(true));
    const query = new URL(calls.find((c) => c.path.startsWith("/api/v1/docs/for-file"))?.path ?? "", "http://x").searchParams;
    expect(query.get("file")).toBe("tx-infinity-api/tx-infinity-core.yml");
  });
});

describe("raising a pull request", () => {
  it("opens the dialog from the button, with both targets checked before anything is created", async () => {
    const { calls } = open();
    await userEvent.click(await screen.findByRole("button", { name: "Raise PR" }));
    const dialog = await screen.findByRole("dialog", { name: "Raise pull request" });
    expect(await within(dialog).findByText("data/config/tx-infinity-api/tx-infinity-core.yml")).toBeInTheDocument();
    expect(within(dialog).getByText("config/tx-infinity-api/tx-infinity-core.yml")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path === "/api/v1/prs")).toBe(false);
  });
});
