import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderNotify } from "@/features/notify/test-utils";
import { expectNoAxeViolations } from "@/test/axe";
import { OfferErrorDialog, OfferLoadingDialog } from "./OfferPhases";
import { OutcomeDialog } from "./OutcomeDialog";
import { SentForApprovalDialog } from "./SentForApprovalDialog";

// The two ways a write that went through is told to the person, inside the dialog that made it (H-3): what the server said it
// did, and that it went to an Admin instead.

describe("OutcomeDialog", () => {
  const show = (over: Partial<React.ComponentProps<typeof OutcomeDialog>> = {}) => {
    const onClose = vi.fn();
    renderNotify(
      <OutcomeDialog open onClose={onClose} tone="success" title="Notification sent" description="The hub accepted 2 paths for sitb." {...over}>
        <p>Audit event 41</p>
      </OutcomeDialog>,
    );
    return onClose;
  };

  it("is named by its title, described by what the server said, and holds what the screen adds", async () => {
    show();
    const dialog = await screen.findByRole("dialog", { name: "Notification sent" });
    expect(dialog).toHaveAccessibleDescription("The hub accepted 2 paths for sitb.");
    expect(within(dialog).getByText("Audit event 41")).toBeInTheDocument();
  });

  it("starts on the dialog itself, so it is read first, and has one Close", async () => {
    show();
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveFocus();
    expect(within(dialog).getAllByRole("button", { name: "Close" })).toHaveLength(1);
  });

  it("closes with the button and with Escape", async () => {
    const onClose = show();
    await userEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it.each(["success", "warning", "info"] as const)("says its %s tone with an icon and the title as well as a colour, and has no axe violations", async (tone) => {
    show({ tone, title: `A ${tone} title` });
    const dialog = await screen.findByRole("dialog", { name: `A ${tone} title` });
    expect(dialog.querySelector(`[data-tone=${tone}] svg`)).not.toBeNull();
    await expectNoAxeViolations();
  });
});

describe("OutcomeDialog actions", () => {
  it("puts the screen's own actions before Close, and Close stays", async () => {
    renderNotify(
      <OutcomeDialog open onClose={() => undefined} tone="warning" title="Couldn't read the offer" description="Check the connection and retry." actions={<button type="button">Retry</button>} />,
    );
    const dialog = await screen.findByRole("dialog");
    const names = within(dialog)
      .getAllByRole("button")
      .map((b) => b.textContent);
    expect(names).toEqual(["Retry", "Close"]);
  });
});

describe("OfferPhases", () => {
  it("says it is loading, as a status that is busy, and can be closed", async () => {
    const onClose = vi.fn();
    renderNotify(<OfferLoadingDialog title="Restart the config-server in sitb?" description="Reading what the restart would take away." what="the config-server restart offer" onClose={onClose} />);
    const dialog = await screen.findByRole("dialog", { name: "Restart the config-server in sitb?" });
    const status = within(dialog).getByRole("status", { name: "Loading the config-server restart offer" });
    expect(status).toHaveAttribute("aria-busy", "true");
    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("says what could not be read, with the request ID, and offers Retry only when it was given one", async () => {
    const onRetry = vi.fn();
    renderNotify(<OfferErrorDialog info={{ title: "Couldn't read the offer", detail: "The server returned an error. Retry.", requestId: "req-1" }} onRetry={onRetry} onClose={() => undefined} />);
    const dialog = await screen.findByRole("dialog", { name: "Couldn't read the offer" });
    expect(dialog).toHaveAccessibleDescription("The server returned an error. Retry.");
    expect(within(dialog).getByText("req-1")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("offers no Retry for a refusal", async () => {
    renderNotify(<OfferErrorDialog info={{ title: "Couldn't read the offer", detail: "Reading it needs the Operator role." }} onClose={() => undefined} />);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("button", { name: "Retry" })).toBeNull();
  });
});

describe("SentForApprovalDialog", () => {
  const show = (canOpenRequest: boolean) => {
    const onClose = vi.fn();
    renderNotify(<SentForApprovalDialog open title="Restart sent for approval" description="Nothing restarts until an Admin approves request AP-12." subject="config-server in sitb" request={12} canOpenRequest={canOpenRequest} onClose={onClose} />);
    return onClose;
  };

  it("says what was sent, names the request, and does not say it was done", async () => {
    show(false);
    const dialog = await screen.findByRole("dialog", { name: "Restart sent for approval" });
    expect(dialog).toHaveAccessibleDescription("Nothing restarts until an Admin approves request AP-12.");
    expect(within(dialog).getByText(/config-server in sitb/)).toBeInTheDocument();
    expect(within(dialog).getByText(/AP-12/, { selector: "p" })).toBeInTheDocument();
  });

  it("offers the request to someone who can open it, and not to anyone else", async () => {
    show(true);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("link", { name: "View request" })).toHaveAttribute("href", "/approvals/12");
  });

  it("offers no link to someone who cannot open it", async () => {
    show(false);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("link")).toBeNull();
  });

  it("closes with Close", async () => {
    const onClose = show(false);
    await userEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
