import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ServedFile } from "@/api/generated/model";
import { binaryServedFile, maskedServedFile, servedFile, withheldServedFile } from "@/mocks/design/served/data";
import { detailOf, lane } from "@/mocks/design/swimlanes/fixtures";
import { expectNoAxeViolations } from "@/test/axe";
import ServedPage from "./ServedPage";
import { createQueryClient } from "@/lib/query/client";
import { problem, queryOf, renderServed, routes, type Api, type Call } from "./test-utils";

// The "As served" page of a swimlane (the page `LinkTemplates::default().served` sends people to): a file as the config-server answers
// for one application, tenant and channel. There is no design for it (H2): it is built from the File view's header and status bar and
// the shared query boundary, and it is a deviation row. What is on trial here is what it asks, when, and what it shows for each answer.

const SELECTION = { file: "tx-infinity-api/connection-pool.yml", application: "tx-infinity-core", tenant: "sit7" };
const DETAIL = detailOf(lane("sitb", { tenants: ["sit7", "sit9"] }));
const SERVED = "GET /api/v1/swimlanes/sitb/served";

const hub = (answer: (call: Call) => unknown = () => servedFile(), detail: unknown = DETAIL): Api =>
  routes({
    "GET /api/v1/swimlanes/sitb": () => detail,
    [SERVED]: answer,
  });
const servedCalls = (calls: Call[]): Call[] => calls.filter((c) => c.path.startsWith("/api/v1/swimlanes/sitb/served"));
const open = (search: Record<string, string> = SELECTION, api: Api = hub()) => renderServed(<ServedPage params={{ id: "sitb" }} search={search} />, { api });

describe("the page", () => {
  it("has its one h1 as soon as it is drawn, before anything has arrived", async () => {
    const held = new Promise<Response>(() => undefined);
    renderServed(<ServedPage params={{ id: "sitb" }} search={SELECTION} />, { api: () => held });
    expect(await screen.findByRole("heading", { level: 1, name: "As served" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("names the swimlane and links back to it", async () => {
    open();
    const link = await screen.findByRole("link", { name: "sitb" });
    expect(link).toHaveAttribute("href", "/swimlanes/sitb");
  });
});

describe("what it asks", () => {
  it("asks the hub for exactly the selection in the address, once", async () => {
    const { calls } = open({ ...SELECTION, channel: "beta" });
    await screen.findByRole("table", { name: /Served text/ });
    expect(servedCalls(calls)).toHaveLength(1);
    expect(Object.fromEntries(queryOf(servedCalls(calls)[0] as Call))).toEqual({ application: "tx-infinity-core", tenant: "sit7", channel: "beta", file: "tx-infinity-api/connection-pool.yml" });
  });

  it("leaves the channel out of the request when the address has none", async () => {
    const { calls } = open();
    await screen.findByRole("table", { name: /Served text/ });
    expect(queryOf(servedCalls(calls)[0] as Call).has("channel")).toBe(false);
  });

  it.each([
    ["nothing", {}],
    ["only a file", { file: SELECTION.file }],
    ["no tenant", { file: SELECTION.file, application: SELECTION.application }],
  ])("asks the config-server nothing for an address with %s", async (_name, search) => {
    const { calls } = open(search);
    await screen.findByText("Choose what to ask for");
    expect(servedCalls(calls)).toHaveLength(0);
  });

  it("asks nothing for an address whose value cannot be valid, and says which field", async () => {
    const { calls } = open({ ...SELECTION, application: "bad name", file: "../etc/passwd" });
    expect(await screen.findByText("That selection can't be asked for")).toBeInTheDocument();
    expect(servedCalls(calls)).toHaveLength(0);
    expect(screen.getByLabelText("Application")).toBeInvalid();
    expect(screen.getByLabelText("File")).toBeInvalid();
  });

  it("asks nothing for a hostile address, and draws it as text", async () => {
    const { calls } = open({ file: '"><img src=x onerror=alert(1)>', application: "javascript:alert(1)", tenant: "<script>" });
    await screen.findByText("That selection can't be asked for");
    expect(servedCalls(calls)).toHaveLength(0);
    expect(document.querySelector("img[src='x']")).toBeNull();
    expect(document.querySelector("script")).toBeNull();
  });
});

describe("the pickers", () => {
  it("fill from the address, and offer the swimlane's tenants", async () => {
    open({ ...SELECTION, channel: "beta" });
    await screen.findByRole("table", { name: /Served text/ });
    expect(screen.getByLabelText("File")).toHaveValue(SELECTION.file);
    expect(screen.getByLabelText("Application")).toHaveValue("tx-infinity-core");
    expect(screen.getByLabelText("Channel")).toHaveValue("beta");
    await waitFor(() => expect(within(screen.getByLabelText("Tenant")).getAllByRole("option").map((o) => o.textContent)).toEqual(["Choose a tenant", "sit7", "sit9"]));
    expect(screen.getByLabelText("Tenant")).toHaveValue("sit7");
  });

  it("go to the address of a new selection, once, and ask for it only when it has arrived there", async () => {
    const view = open();
    await screen.findByRole("table", { name: /Served text/ });
    await waitFor(() => expect(within(screen.getByLabelText("Tenant")).getAllByRole("option")).toHaveLength(3));
    await userEvent.selectOptions(screen.getByLabelText("Tenant"), "sit9");
    expect(servedCalls(view.calls)).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(view.navigations).toEqual(["/swimlanes/sitb/served?application=tx-infinity-core&file=tx-infinity-api%2Fconnection-pool.yml&tenant=sit9"]);
    expect(servedCalls(view.calls)).toHaveLength(1);
  });

  it("ask again, without moving, when the selection did not change", async () => {
    const view = open();
    await screen.findByRole("table", { name: /Served text/ });
    await waitFor(() => expect(within(screen.getByLabelText("Tenant")).getAllByRole("option")).toHaveLength(3));
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    await waitFor(() => expect(servedCalls(view.calls)).toHaveLength(2));
    expect(view.navigations).toEqual([]);
  });

  it("send nothing while the fields are incomplete", async () => {
    const view = open({});
    await screen.findByText("Choose what to ask for");
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(view.navigations).toEqual([]);
    expect(servedCalls(view.calls)).toHaveLength(0);
    expect(screen.getByText("A file is required")).toBeInTheDocument();
  });

  it("let the tenant be typed when the swimlane could not be read", async () => {
    open(SELECTION, routes({ [SERVED]: () => servedFile() }));
    await screen.findByRole("table", { name: /Served text/ });
    expect(screen.getByLabelText("Tenant").tagName).toBe("INPUT");
  });
});

describe("what it shows", () => {
  it("shows the text the config-server served", async () => {
    open();
    const table = await screen.findByRole("table", { name: "Served text of connection-pool.yml" });
    expect(within(table).getAllByRole("row")).toHaveLength(8);
    expect(screen.getByText("1 placeholder left as written")).toBeInTheDocument();
  });

  it("shows a masked text with hidden values, no Reveal, and no value anywhere", async () => {
    open(SELECTION, hub(() => maskedServedFile()));
    await screen.findByRole("table", { name: /Served text/ });
    expect(document.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    expect(screen.queryByRole("button", { name: /reveal/i })).toBeNull();
  });

  it("shows no text and no copy of the hash for a file the hub withholds", async () => {
    open({ ...SELECTION, file: "security/keystore.jks" }, hub(() => withheldServedFile()));
    expect(await screen.findByText("Content withheld")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("button", { name: "Copy hash" })).toBeNull();
  });

  it("shows that an answer is not text, and not an error", async () => {
    open({ ...SELECTION, file: "tx-infinity-api/logo.png" }, hub(() => binaryServedFile()));
    expect(await screen.findByText("Not shown as text")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows no way to copy the text, to edit it, or to reveal anything in it", async () => {
    open(SELECTION, hub(() => maskedServedFile()));
    await screen.findByRole("table", { name: /Served text/ });
    expect(screen.queryByRole("button", { name: /^copy (text|all|file|served)/i })).toBeNull();
    expect(screen.queryByRole("textbox", { name: /text/i })).toBeNull();
    for (const name of [/edit/i, /save/i, /reveal/i, /download/i, /raise pr/i]) expect(screen.queryByRole("button", { name }), String(name)).toBeNull();
  });

  it("says not served for a 404, with the request id, and asks again on request", async () => {
    let asked = 0;
    const view = open(
      SELECTION,
      hub(() => {
        asked += 1;
        return problem(404, "not_found");
      }),
    );
    expect(await screen.findByText("Not served for this selection")).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Ask again" }));
    await waitFor(() => expect(asked).toBe(2));
    expect(servedCalls(view.calls)).toHaveLength(2);
  });

  it("says a refused read is a refused read: the request id, no Retry, and one request", async () => {
    const view = open(SELECTION, hub(() => problem(403, "forbidden")));
    expect(await screen.findByText(/you do not have access/i)).toBeInTheDocument();
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(servedCalls(view.calls)).toHaveLength(1);
  });

  it("does not retry a 503 by itself: it shows the error at once with Retry, and Retry asks again and shows the text", async () => {
    // Each request is a rate-limited round trip to an agent, so the query is not tried again on its own (queries.ts, `retry: false`).
    let up = false;
    const view = open(SELECTION, hub(() => (up ? servedFile() : problem(503, "service_unavailable"))));
    expect(await screen.findByText("Couldn't load the served file", {}, { timeout: 2_000 })).toBeInTheDocument();
    expect(servedCalls(view.calls)).toHaveLength(1);
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    up = true;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("table", { name: /Served text/ })).toBeInTheDocument();
    expect(servedCalls(view.calls)).toHaveLength(2);
  }, 30_000);

  it("says the agent is offline when the swimlane says so", async () => {
    open(SELECTION, hub(() => servedFile(), detailOf(lane("sitb", { agent: "disconnected", tenants: ["sit7"] }))));
    expect(await screen.findByText("Agent for sitb is offline")).toBeInTheDocument();
  });
});

describe("the answer in the page and the address", () => {
  it("takes a new selection from the address (Back, a link) and asks for that one", async () => {
    const answers: Record<string, ServedFile> = { sit7: servedFile(), sit9: servedFile({ tenant: "sit9", hash: "a".repeat(64) }) };
    const view = open(SELECTION, hub((call) => answers[queryOf(call).get("tenant") ?? ""]));
    await screen.findByRole("table", { name: /Served text/ });
    view.update(<ServedPage params={{ id: "sitb" }} search={{ ...SELECTION, tenant: "sit9" }} />);
    await waitFor(() => expect(servedCalls(view.calls)).toHaveLength(2));
    expect(queryOf(servedCalls(view.calls)[1] as Call).get("tenant")).toBe("sit9");
    await waitFor(() => expect(screen.getByLabelText("Tenant")).toHaveValue("sit9"));
  });
});

describe("accessibility", () => {
  it("has no structural violation with an answer", async () => {
    open();
    await screen.findByRole("table", { name: /Served text/ });
    await expectNoAxeViolations();
  });
  it("has none with nothing chosen", async () => {
    open({});
    await screen.findByText("Choose what to ask for");
    await expectNoAxeViolations();
  });
});

describe("coming back to the page", () => {
  it("asks the config-server again whenever the page is opened, because the answer is live and an edit just made may be in it", async () => {
    let asked = 0;
    const client = createQueryClient();
    const api = hub(() => {
      asked += 1;
      return servedFile();
    });
    const first = renderServed(<ServedPage params={{ id: "sitb" }} search={SELECTION} />, { api, client });
    await screen.findByRole("table", { name: /Served text/ });
    expect(asked).toBe(1);
    first.unmount();
    renderServed(<ServedPage params={{ id: "sitb" }} search={SELECTION} />, { api, client });
    // The answer from a moment ago is on screen at once, and the page does not trust it.
    await screen.findByRole("table", { name: /Served text/ });
    await waitFor(() => expect(asked).toBe(2));
  });
});

describe("what the page keeps", () => {
  it("keeps the served text in the query cache only, which lets go of it half a minute after the page is left", async () => {
    const view = open();
    await screen.findByRole("table", { name: /Served text/ });
    const entries = view.client.getQueryCache().findAll({ queryKey: ["served"] });
    expect(entries).toHaveLength(1);
    expect(entries[0]?.options.gcTime).toBe(30_000);
  });
});
