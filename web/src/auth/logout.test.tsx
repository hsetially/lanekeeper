import { afterEach, describe, expect, it } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { apiCaches } from "@/api/client";
import { json, me, problem, renderApp, TOKEN } from "@/test/app";

afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

describe("sign out", () => {
  it("posts to /auth/logout with the CSRF token, empties the caches and goes to /signin", async () => {
    apiCaches.etags.set("/api/v1/x", '"a"', "{}");
    const app = renderApp("/swimlanes", (call) => {
      if (call.path === "/api/v1/me") return json(me("viewer"));
      if (call.path === "/auth/logout") return new Response(null, { status: 204 });
      return problem(404, "not_found");
    });
    await userEvent.click(await screen.findByRole("button", { name: "Sign out" }));
    await waitFor(() => expect(app.location()).toBe("/signin"));
    const logout = app.calls.find((c) => c.path === "/auth/logout");
    expect(logout?.method).toBe("POST");
    expect(logout?.headers.get("x-csrf-token")).toBe(TOKEN);
    expect(app.queryClient.getQueryCache().getAll().filter((q) => q.state.data !== undefined)).toHaveLength(0);
    expect(apiCaches.etags.size).toBe(0);
  });

  it("stays where it is and says so when the hub does not confirm", async () => {
    const app = renderApp("/swimlanes", (call) => {
      if (call.path === "/api/v1/me") return json(me("viewer"));
      if (call.path === "/auth/logout") return problem(500, "internal");
      return problem(404, "not_found");
    });
    await userEvent.click(await screen.findByRole("button", { name: "Sign out" }));
    expect(await screen.findByText("Could not sign out")).toBeInTheDocument();
    expect(app.location()).toBe("/swimlanes");
  });
});
