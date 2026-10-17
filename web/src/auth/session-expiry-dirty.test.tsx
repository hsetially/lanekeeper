import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, screen, waitFor } from "@testing-library/react";
import { apiFetch } from "@/api/client";
import { json, me, problem, renderApp } from "@/test/app";
import { clearDirtyForTests, markDirty } from "./dirty";

// A 401 while there is unsaved work must not navigate away: the page stays, says the session ended, and offers the draft.
beforeEach(() => clearDirtyForTests());
afterEach(() => {
  clearDirtyForTests();
  Reflect.deleteProperty(document, "cookie");
});

const api = (call: { path: string }) => {
  if (call.path === "/api/v1/me") return json(me("editor"));
  if (call.path === "/api/v1/swimlanes/sit1/files/content") return problem(401, "unauthenticated");
  return problem(404, "not_found");
};

describe("session expiry with unsaved work", () => {
  it("stays on the page, says the session ended, and offers to copy the draft", async () => {
    const app = renderApp("/swimlanes/sit1/files?path=a.yml", api);
    await screen.findByRole("heading", { level: 1, name: "Files" });
    markDirty("editor", { getDraft: () => "my unsaved text" });
    await act(async () => {
      await apiFetch("/api/v1/swimlanes/sit1/files/content").catch(() => undefined);
    });
    expect(await screen.findByText("Your session ended")).toBeInTheDocument();
    expect(app.location()).toBe("/swimlanes/sit1/files?path=a.yml");
    expect(screen.getByRole("link", { name: "Sign in again" })).toHaveAttribute("href", "/auth/login?return_to=%2Fswimlanes%2Fsit1%2Ffiles%3Fpath%3Da.yml");
    expect(screen.getByRole("button", { name: /copy draft/i })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Files" })).toBeInTheDocument();
  });

  it("goes to sign-in when nothing is unsaved", async () => {
    const app = renderApp("/swimlanes/sit1/files?path=a.yml", api);
    await screen.findByRole("heading", { level: 1, name: "Files" });
    await act(async () => {
      await apiFetch("/api/v1/swimlanes/sit1/files/content").catch(() => undefined);
    });
    await waitFor(() => expect(app.location()).toMatch(/^\/signin\?next=/));
    expect(screen.queryByText("Your session ended")).not.toBeInTheDocument();
  });

  it("offers no copy button when nothing has a draft", async () => {
    renderApp("/swimlanes/sit1/files", api);
    await screen.findByRole("heading", { level: 1, name: "Files" });
    markDirty("dialog");
    await act(async () => {
      await apiFetch("/api/v1/swimlanes/sit1/files/content").catch(() => undefined);
    });
    expect(await screen.findByText("Your session ended")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /copy draft/i })).not.toBeInTheDocument();
  });
});
