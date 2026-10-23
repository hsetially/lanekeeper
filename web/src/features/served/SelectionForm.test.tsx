import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { readSelection, type ServedDraft } from "./model";
import { SelectionForm } from "./SelectionForm";
import { renderServed } from "./test-utils";

// The pickers of the "As served" view: the file, the application, the tenant and the channel, and one button. Nothing is asked of
// the config-server until the button is pressed (each request is a round trip through the swimlane's agent), and nothing is sent that
// cannot be a request. The tenant is chosen from the swimlane's own when they are known.

const EMPTY: ServedDraft = { file: "", application: "", tenant: "", channel: "" };
const FULL: ServedDraft = { file: "tx-infinity-api/connection-pool.yml", application: "tx-infinity-core", tenant: "sit7", channel: "" };

type Over = Partial<Parameters<typeof SelectionForm>[0]>;
async function open(over: Over = {}) {
  const onShow = vi.fn();
  const view = renderServed(<SelectionForm draft={EMPTY} problems={{}} tenants={["sit7", "sit9"]} busy={false} onShow={onShow} {...over} />);
  await screen.findByRole("form", { name: "What to ask the config-server for" });
  return { ...view, onShow };
}

describe("the fields", () => {
  it("has a labelled field for the file, the application, the tenant and the channel, and a button", async () => {
    await open();
    for (const name of ["File", "Application", "Tenant", "Channel"]) expect(screen.getByLabelText(name)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show served text" })).toBeInTheDocument();
  });

  it("starts with what the address holds", async () => {
    await open({ draft: { ...FULL, channel: "beta" } });
    expect(screen.getByLabelText("File")).toHaveValue(FULL.file);
    expect(screen.getByLabelText("Application")).toHaveValue("tx-infinity-core");
    expect(screen.getByLabelText("Tenant")).toHaveValue("sit7");
    expect(screen.getByLabelText("Channel")).toHaveValue("beta");
  });

  it("takes the address again when it changes (Back, a link), and keeps what the person typed until then", async () => {
    const onShow = vi.fn();
    const first = renderServed(<SelectionForm draft={FULL} problems={{}} tenants={["sit7", "sit9"]} busy={false} onShow={onShow} />);
    const application = await screen.findByLabelText("Application");
    await userEvent.clear(application);
    await userEvent.type(application, "typed");
    first.update(<SelectionForm draft={FULL} problems={{}} tenants={["sit7", "sit9"]} busy={false} onShow={onShow} />);
    expect(screen.getByLabelText("Application")).toHaveValue("typed");
    first.update(<SelectionForm draft={{ ...FULL, application: "other-app" }} problems={{}} tenants={["sit7", "sit9"]} busy={false} onShow={onShow} />);
    await waitFor(() => expect(screen.getByLabelText("Application")).toHaveValue("other-app"));
  });

  it("explains the application and the channel once, for everyone who reads the form", async () => {
    await open();
    expect(screen.getByText(/name a service asks the config-server for/i)).toBeInTheDocument();
    expect(screen.getByText(/leave the channel empty/i)).toBeInTheDocument();
  });
});

describe("the tenant", () => {
  it("is chosen from the swimlane's tenants", async () => {
    await open();
    const select = screen.getByLabelText("Tenant");
    expect(select.tagName).toBe("SELECT");
    expect(within(select).getAllByRole("option").map((o) => o.textContent)).toEqual(["Choose a tenant", "sit7", "sit9"]);
  });

  it("is typed when the swimlane's tenants are not known (not loaded, or none yet)", async () => {
    await open({ tenants: [] });
    expect(screen.getByLabelText("Tenant").tagName).toBe("INPUT");
  });

  it("keeps a tenant from the address that the swimlane does not have, and says so, so it can be seen and changed", async () => {
    await open({ draft: { ...FULL, tenant: "sit99" } });
    const select = screen.getByLabelText("Tenant");
    expect(select).toHaveValue("sit99");
    expect(within(select).getByRole("option", { name: /sit99 \(not a tenant of this swimlane\)/ })).toBeInTheDocument();
  });

  it("shows a hidden character in such a tenant as a visible marker", async () => {
    await open({ draft: { ...FULL, tenant: "a‮b" }, tenants: ["sit7"] });
    const option = within(screen.getByLabelText("Tenant")).getByRole("option", { name: /not a tenant/ });
    expect(option.textContent).toContain("<U+202E>");
    expect(option.textContent).not.toContain("‮");
  });
});

describe("showing the text", () => {
  it("sends the selection, trimmed, with no channel as null", async () => {
    const { onShow } = await open({ draft: FULL });
    await userEvent.clear(screen.getByLabelText("Application"));
    await userEvent.type(screen.getByLabelText("Application"), "  tx-infinity-core ");
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(onShow).toHaveBeenCalledTimes(1);
    expect(onShow).toHaveBeenCalledWith({ file: FULL.file, application: "tx-infinity-core", tenant: "sit7", channel: null });
  });

  it("sends the channel when there is one", async () => {
    const { onShow } = await open({ draft: FULL });
    await userEvent.type(screen.getByLabelText("Channel"), "beta");
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(onShow).toHaveBeenCalledWith({ ...FULL, channel: "beta" });
  });

  it("is sent by Enter in a field, as well as by the button", async () => {
    const { onShow } = await open({ draft: FULL });
    await userEvent.type(screen.getByLabelText("Application"), "{Enter}");
    expect(onShow).toHaveBeenCalledTimes(1);
  });

  it("sends nothing until the button is pressed: typing and choosing ask nothing", async () => {
    const { onShow } = await open({ draft: FULL });
    await userEvent.type(screen.getByLabelText("Application"), "x");
    await userEvent.selectOptions(screen.getByLabelText("Tenant"), "sit9");
    expect(onShow).not.toHaveBeenCalled();
  });

  it("sends the tenant that was chosen", async () => {
    const { onShow } = await open({ draft: FULL });
    await userEvent.selectOptions(screen.getByLabelText("Tenant"), "sit9");
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(onShow).toHaveBeenCalledWith({ ...FULL, tenant: "sit9", channel: null });
  });

  it("keeps the button the width of its longer label while a request is out, so the fields beside it do not move (found by the T6 visual run: the form reflowed by 45 px)", async () => {
    const idle = await open({ draft: FULL });
    const idleButton = screen.getByRole("button", { name: "Show served text" });
    expect(idleButton).toHaveTextContent("Asking the config-server…");
    idle.unmount();
    await open({ draft: FULL, busy: true });
    const busyButton = screen.getByRole("button", { name: "Asking the config-server…" });
    // Both labels are in the button, one on top of the other; the one that is not said is invisible and out of the accessibility tree, so
    // the name, the width and the position are the same in both states.
    expect(busyButton).toHaveTextContent("Show served text");
    const hidden = within(busyButton).getByText("Show served text");
    expect(hidden).toHaveAttribute("aria-hidden", "true");
    expect(hidden.className).toMatch(/(^|\s)invisible(\s|$)/);
    expect(hidden.className).toMatch(/(^|\s)col-start-1(\s|$)/);
    expect(within(busyButton).getByText("Asking the config-server…").className).toMatch(/(^|\s)col-start-1(\s|$)/);
  });

  it("does not send a second time while it is asking", async () => {
    const { onShow } = await open({ draft: FULL, busy: true });
    await userEvent.click(screen.getByRole("button", { name: "Asking the config-server…" }));
    expect(onShow).not.toHaveBeenCalled();
  });
});

describe("a form that cannot be sent", () => {
  it("names what is missing next to each field, and sends nothing", async () => {
    const { onShow } = await open();
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(onShow).not.toHaveBeenCalled();
    expect(screen.getByLabelText("File")).toBeInvalid();
    expect(screen.getByText("A file is required")).toBeInTheDocument();
    expect(screen.getByText("An application is required")).toBeInTheDocument();
    expect(screen.getByText("A tenant is required")).toBeInTheDocument();
    expect(screen.getByLabelText("Channel")).not.toBeInvalid();
  });

  it("puts the focus on the first field that is wrong", async () => {
    await open({ draft: { ...FULL, application: "" } });
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(screen.getByLabelText("Application")).toHaveFocus();
  });

  it("says what a path, a name or a channel must look like, and never repeats what was typed", async () => {
    const { onShow } = await open({ draft: { file: "../etc/hunter2", application: "bad name hunter2", tenant: "sit7", channel: "-hunter2" } });
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(onShow).not.toHaveBeenCalled();
    expect(screen.getByText(/not a path below the swimlane's root/i)).toBeInTheDocument();
    expect(screen.getAllByText(/letters, digits/i)).toHaveLength(2);
    expect(screen.getAllByRole("alert").map((a) => a.textContent).join(" ")).not.toContain("hunter2");
  });

  it("checks again while the person fixes it, and sends when it is right", async () => {
    const { onShow } = await open({ draft: { ...FULL, application: "bad name" } });
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(screen.getByLabelText("Application")).toBeInvalid();
    await userEvent.clear(screen.getByLabelText("Application"));
    await userEvent.type(screen.getByLabelText("Application"), "tx-infinity-core");
    expect(screen.getByLabelText("Application")).not.toBeInvalid();
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    expect(onShow).toHaveBeenCalledTimes(1);
  });

  it("shows the problems of an address that gave a value that cannot be valid, before anything is pressed", async () => {
    const read = readSelection({ ...FULL, application: "bad name", file: "../x" });
    await open({ draft: read.draft, problems: read.problems });
    expect(screen.getByLabelText("Application")).toBeInvalid();
    expect(screen.getByLabelText("File")).toBeInvalid();
    expect(screen.getByLabelText("Tenant")).not.toBeInvalid();
  });
});

describe("a hostile value", () => {
  it("is put in a field as text, never as markup", async () => {
    await open({ draft: { ...FULL, file: '"><img src=x onerror=alert(1)>' } });
    expect(document.querySelector("img[src='x']")).toBeNull();
    expect(screen.getByLabelText("File")).toHaveValue('"><img src=x onerror=alert(1)>');
  });
});

describe("accessibility", () => {
  it("has no structural violation, with and without problems", async () => {
    await open({ draft: FULL });
    await expectNoAxeViolations();
  });
  it("has none with errors showing", async () => {
    await open();
    await userEvent.click(screen.getByRole("button", { name: "Show served text" }));
    await expectNoAxeViolations();
  });
});
