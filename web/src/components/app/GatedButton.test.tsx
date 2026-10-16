import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { GatedButton } from "./GatedButton";
import { WriteGateProvider, useWriteGate } from "./write-gate";

const ROLES = ["viewer", "editor", "operator", "admin"] as const;

function withGate(role: string | null, online = true) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <WriteGateProvider value={{ roles: ROLES, role, online }}>{children}</WriteGateProvider>
    );
  };
}

function Probe({ minRole, needsConnection }: { minRole?: string; needsConnection?: boolean }) {
  const gate = useWriteGate(minRole, needsConnection === undefined ? {} : { needsConnection });
  return <output data-testid="gate">{JSON.stringify(gate)}</output>;
}

describe("useWriteGate", () => {
  const read = (): { allowed: boolean; reason: string | null } => JSON.parse(screen.getByTestId("gate").textContent ?? "null");

  it("allows a role at or above the minimum", () => {
    render(<Probe minRole="operator" />, { wrapper: withGate("admin") });
    expect(read()).toEqual({ allowed: true, reason: null });
  });

  it("names the role needed and who can grant it", () => {
    render(<Probe minRole="operator" />, { wrapper: withGate("editor") });
    expect(read()).toEqual({ allowed: false, reason: "Needs the Operator role. Ask an Admin to grant it." });
  });

  it("explains a missing role, not a signed-in one with too little", () => {
    render(<Probe minRole="editor" />, { wrapper: withGate(null) });
    const { allowed, reason } = read();
    expect(allowed).toBe(false);
    expect(reason).toMatch(/no role/i);
    expect(reason).toMatch(/Admin/);
  });

  it("is disabled offline with the one reason the handoff gives", () => {
    render(<Probe minRole="viewer" />, { wrapper: withGate("admin", false) });
    expect(read()).toEqual({ allowed: false, reason: "Unavailable offline" });
  });

  it("states the permission reason before the temporary offline one", () => {
    render(<Probe minRole="operator" />, { wrapper: withGate("viewer", false) });
    expect(read().reason).toMatch(/Operator role/);
  });

  it("lets a read-only action ignore the connection", () => {
    render(<Probe minRole="viewer" needsConnection={false} />, { wrapper: withGate("viewer", false) });
    expect(read().allowed).toBe(true);
  });

  it("refuses everything when no provider is mounted (fails closed)", () => {
    render(<Probe minRole="viewer" />);
    expect(read().allowed).toBe(false);
  });

  it("refuses an unknown minimum role instead of allowing it", () => {
    render(<Probe minRole="superuser" />, { wrapper: withGate("admin") });
    expect(read().allowed).toBe(false);
  });
});

describe("GatedButton", () => {
  it("works like a button when the gate allows it", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(<GatedButton gate={{ allowed: true, reason: null }} onClick={onClick}>Restart</GatedButton>);
    await user.click(screen.getByRole("button", { name: "Restart" }));
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Restart" })).not.toHaveAttribute("aria-disabled");
  });

  it("is aria-disabled, focusable and silent when the gate refuses, and says why", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    const reason = "Needs the Operator role. Ask an Admin to grant it.";
    render(<GatedButton gate={{ allowed: false, reason }} onClick={onClick}>Restart</GatedButton>);
    const button = screen.getByRole("button", { name: "Restart" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).not.toBeDisabled();
    expect(button).toHaveAccessibleDescription(reason);
    await user.tab();
    expect(button).toHaveFocus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent(reason);
    await user.keyboard("{Enter}");
    await user.click(button);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("takes the role from the provider when given minRole", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(<GatedButton minRole="operator" onClick={onClick}>Restart</GatedButton>, { wrapper: withGate("editor") });
    const button = screen.getByRole("button", { name: "Restart" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    await user.click(button);
    expect(onClick).not.toHaveBeenCalled();
    await waitFor(() => expect(button).toHaveAccessibleDescription(/Operator role/));
  });
});
