import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Role } from "@/api/generated/model";
import { ROLE_ORDER } from "@/auth/roles";
import { SessionContext, type SessionState } from "@/auth/session";
import { PermissionState } from "@/components/app/PermissionState";
import { WriteGateProvider } from "@/components/app/write-gate";
import { createQueryClient } from "@/lib/query/client";
import { meBody } from "@/mocks/me";
import RoleChangeAction from "./RoleChangeAction";

// The permission state's action (States index: "Admin needs the Admin role ... [Request a role change]") for any screen that shows
// one: it reads the person's role from the session, and is drawn only for a person who can ask.

afterEach(() => {
  vi.unstubAllGlobals();
});

function ready(role: Role | null): SessionState {
  return { status: "ready", me: meBody(role), role };
}

function mount(session: SessionState, need: Role | null = Role.admin) {
  return render(
    <QueryClientProvider client={createQueryClient()}>
      <SessionContext.Provider value={session}>
        <WriteGateProvider value={{ roles: ROLE_ORDER, role: session.status === "ready" ? session.role : null, online: true }}>
          <PermissionState what="Admin" need={need} have={session.status === "ready" ? session.role : null} as="h1" action={<RoleChangeAction need={need} />} />
        </WriteGateProvider>
      </SessionContext.Provider>
    </QueryClientProvider>,
  );
}

describe("RoleChangeAction", () => {
  it("is the button of the permission state for a Viewer, and opens the dialog with the role the screen needed", async () => {
    const user = userEvent.setup();
    mount(ready(Role.viewer));
    expect(screen.getByRole("heading", { level: 1, name: "Admin needs the Admin role" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Request a role change" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Request a role change" });
    expect(within(dialog).getByRole("radio", { name: /^Admin/ })).toBeChecked();
  });

  it("is drawn for an Editor and an Operator too", () => {
    for (const role of [Role.editor, Role.operator]) {
      const view = mount(ready(role));
      expect(screen.getByRole("button", { name: "Request a role change" })).toBeInTheDocument();
      view.unmount();
    }
  });

  it("is not drawn for an Admin, for a person with no role, or before the session is known", () => {
    for (const session of [ready(Role.admin), ready(null), { status: "loading" } as const, { status: "none" } as const]) {
      const view = mount(session);
      expect(screen.queryByRole("button", { name: "Request a role change" }), JSON.stringify(session.status)).toBeNull();
      view.unmount();
    }
  });
});
