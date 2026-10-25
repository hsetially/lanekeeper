import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { proposal, ANA } from "@/mocks/design/approvals/fixtures";
import { canDecide } from "./model";
import { DecisionFooter, MAX_REASON, MIN_REASON } from "./DecisionFooter";

// The foot of the drawer on its own: which button can be pressed, and the reason it gives when it can not. The permission and the
// connection come first, then what is true of the proposal, then the reason field.

const ME = "11111111-1111-4111-8111-111111111111:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const OPEN = { allowed: true, reason: null } as const;

function footer(over: Partial<Parameters<typeof DecisionFooter>[0]> = {}) {
  const p = proposal({ id: 1, author: ANA, swimlane: "sitb", path: "a/b.yml" });
  const props = {
    p,
    authorName: "Ana Ruiz",
    decide: canDecide(p, ME),
    base: OPEN,
    reason: "",
    onReason: vi.fn(),
    working: null,
    onApprove: vi.fn(),
    onReject: vi.fn(),
    error: null,
    ...over,
  } satisfies Parameters<typeof DecisionFooter>[0];
  render(<DecisionFooter {...props} />);
  return props;
}

describe("the decision footer", () => {
  it("asks for a reason by name, for the person it is addressed to, up to what the contract takes", () => {
    footer();
    const field = screen.getByLabelText("Reason (required to reject)");
    expect(field).toHaveAttribute("placeholder", "Tell Ana Ruiz what to change");
    expect(field).toHaveAttribute("maxlength", String(MAX_REASON));
    expect(MIN_REASON).toBe(3);
  });

  it("lets Approve through at once and Reject once there is a reason of three characters", async () => {
    const first = footer();
    expect(screen.getByRole("button", { name: "Reject" })).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(screen.getByRole("button", { name: "Approve" }));
    expect(first.onApprove).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Reject" }));
    expect(first.onReject).not.toHaveBeenCalled();
  });

  it("does not count spaces as a reason", () => {
    footer({ reason: "ab    " });
    expect(screen.getByRole("button", { name: "Reject" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("Write a reason to reject")).toBeInTheDocument();
  });

  it("sends the reject once the reason is there", async () => {
    const props = footer({ reason: "abc" });
    await userEvent.click(screen.getByRole("button", { name: "Reject" }));
    expect(props.onReject).toHaveBeenCalledTimes(1);
  });

  it("says the connection or the role first, whatever is true of the proposal", () => {
    footer({ base: { allowed: false, reason: "Unavailable offline" }, reason: "abc" });
    expect(screen.getByRole("button", { name: "Approve" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Reject" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getAllByText("Unavailable offline")).toHaveLength(2);
  });

  it("offers neither button, and no reason field, to the author of the proposal", () => {
    const p = proposal({ id: 2, author: ME, swimlane: "sitb", path: "a/b.yml" });
    footer({ p, decide: canDecide(p, ME) });
    expect(screen.queryByLabelText("Reason (required to reject)")).toBeNull();
    expect(screen.getByRole("button", { name: "Approve" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Reject" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("You can't approve your own proposal")).toBeInTheDocument();
  });

  it("shows only one decision at a time: the other button waits, and the field is locked", () => {
    footer({ working: "approve", reason: "abc" });
    expect(screen.getByRole("button", { name: "Approving…" })).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "Reject" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("A decision is being recorded")).toBeInTheDocument();
    expect(screen.getByLabelText("Reason (required to reject)")).toBeDisabled();
  });

  it("shows the refusal with its request ID above the buttons", () => {
    footer({ error: { title: "You may not decide proposals", detail: "Deciding needs the Admin role. Nothing was recorded.", requestId: "req-9" } });
    expect(screen.getByRole("alert")).toHaveTextContent("You may not decide proposals");
    expect(screen.getByText("req-9")).toBeInTheDocument();
  });
});
