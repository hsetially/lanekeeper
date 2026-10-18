import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { apiCaches } from "@/api/client";
import { createQueryClient } from "@/lib/query/client";
import { UPLOAD_DOC } from "@/mocks/design/search/data";
import { DeleteDocDialog } from "./DeleteDocDialog";

// Delete is the one destructive write of the docs: the button stays off until the file's name is typed, Enter never confirms,
// the write is sent once with a CSRF token and an Idempotency-Key, and a failure stays in the dialog with the request id.

type Sent = { method: string; path: string; headers: Headers };
let sent: Sent[];
let answer: () => Response;
const fail = (status: number, type: string) => new Response(JSON.stringify({ type: `urn:lanekeeper:problem:${type}`, title: "x", status, request_id: "req-del-1" }), { status, headers: { "content-type": "application/problem+json" } });

beforeEach(() => {
  apiCaches.clear();
  Object.defineProperty(document, "cookie", { configurable: true, get: () => "__Host-lk_csrf=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde", set: () => undefined });
  sent = [];
  answer = () => new Response(null, { status: 204 });
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      sent.push({ method: (init.method ?? "GET").toUpperCase(), path: input, headers: new Headers(init.headers) });
      return Promise.resolve(answer());
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

function mount() {
  const onOpenChange = vi.fn();
  const onDone = vi.fn();
  render(
    <QueryClientProvider client={createQueryClient()}>
      <DeleteDocDialog open onOpenChange={onOpenChange} doc={UPLOAD_DOC} onDone={onDone} />
    </QueryClientProvider>,
  );
  return { onOpenChange, onDone };
}

describe("delete a doc", () => {
  it("says what is lost first, and asks for the name of the file", () => {
    mount();
    const dialog = screen.getByRole("alertdialog", { name: "Delete “Instant payments”?" });
    expect(dialog).toHaveTextContent("Removes the doc for everyone");
    expect(dialog).toHaveTextContent("Links to it stop working");
    expect(screen.getByLabelText(/Type instant-payments\.md to confirm/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete doc" })).toHaveAttribute("aria-disabled", "true");
  });

  it("sends nothing until the name is typed exactly, and never on Enter", async () => {
    mount();
    const field = screen.getByLabelText(/Type instant-payments\.md to confirm/);
    await userEvent.type(field, "instant-payments{Enter}");
    await userEvent.click(screen.getByRole("button", { name: "Delete doc" }));
    expect(sent).toEqual([]);
    await userEvent.type(field, ".md{Enter}");
    expect(sent).toEqual([]);
    expect(screen.getByRole("button", { name: "Delete doc" })).not.toHaveAttribute("aria-disabled");
  });

  it("deletes the doc by its id, with a CSRF token and an Idempotency-Key, then hands on and closes", async () => {
    const { onDone, onOpenChange } = mount();
    await userEvent.type(screen.getByLabelText(/Type instant-payments\.md to confirm/), "instant-payments.md");
    await userEvent.click(screen.getByRole("button", { name: "Delete doc" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({ method: "DELETE", path: "/api/v1/docs/102" });
    expect(sent[0]?.headers.get("x-csrf-token")).toBe("AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde");
    expect(sent[0]?.headers.get("idempotency-key")).toMatch(/^[A-Za-z0-9._-]{1,128}$/);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it.each([
    [404, "not_found", "This doc no longer exists"],
    [403, "forbidden", "You do not have the Editor role"],
    [500, "internal", "The server could not delete the doc"],
  ])("stays open on a %i and says nothing was deleted, with the request id", async (status, type, text) => {
    answer = () => fail(status, type);
    const { onDone, onOpenChange } = mount();
    await userEvent.type(screen.getByLabelText(/Type instant-payments\.md to confirm/), "instant-payments.md");
    await userEvent.click(screen.getByRole("button", { name: "Delete doc" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(text);
    expect(alert).toHaveTextContent("Nothing was deleted");
    expect(alert).toHaveTextContent("req-del-1");
    expect(onDone).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });
});
