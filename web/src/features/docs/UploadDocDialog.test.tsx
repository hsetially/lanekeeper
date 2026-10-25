import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { apiCaches } from "@/api/client";
import type { DocSummary } from "@/api/generated/model";
import { createQueryClient } from "@/lib/query/client";
import { UPLOAD_DOC, NOW } from "@/mocks/design/search/data";
import { UploadDocDialog } from "./UploadDocDialog";

// Upload and replace: the drop zone and its three states (ready, wrong type, over 2 MB), the consequences before the button,
// the write with its expected version, and what the dialog does with each answer: it stays open on a failure, with the file
// and the title the person chose, and says what happened with the request id.

type Sent = { method: string; path: string; body: unknown; headers: Headers };
let sent: Sent[];
let client: QueryClient;
let answer: () => Response;

const ok = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const fail = (status: number, type: string, title: string) => new Response(JSON.stringify({ type: `urn:lanekeeper:problem:${type}`, title, status, request_id: "req-doc-1" }), { status, headers: { "content-type": "application/problem+json" } });
const md = (text: string, name = "instant-payments.md") => new File([text], name, { type: "text/markdown" });

beforeEach(() => {
  apiCaches.clear();
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  Object.defineProperty(document, "cookie", { configurable: true, get: () => "__Host-lk_csrf=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde", set: () => undefined });
  sent = [];
  answer = () => ok({ ...UPLOAD_DOC, version: 4 }, 200);
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      sent.push({ method: (init.method ?? "GET").toUpperCase(), path: input, body: typeof init.body === "string" ? (JSON.parse(init.body) as unknown) : undefined, headers: new Headers(init.headers) });
      return Promise.resolve(answer());
    }),
  );
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "cookie");
});

type Mode = { kind: "create" } | { kind: "replace"; doc: DocSummary };
function mount(mode: Mode) {
  client = createQueryClient();
  const onOpenChange = vi.fn();
  const onDone = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <UploadDocDialog open onOpenChange={onOpenChange} mode={mode} onDone={onDone} />
    </QueryClientProvider>,
  );
  return { onOpenChange, onDone };
}

const chooser = () => document.querySelector<HTMLInputElement>("input[type=file]") as HTMLInputElement;
// `delay: null`: the tests keep Date fake, and user-event's default timers would wait on a clock that does not run.
// `applyAccept: false`: a file picker offers Markdown files, but a drop does not filter, so a wrong file can reach the dialog.
const user = () => userEvent.setup({ delay: null, applyAccept: false });

describe("upload", () => {
  it("opens on the drop zone, with the limits and a button that waits for a file", () => {
    mount({ kind: "create" });
    const dialog = screen.getByRole("alertdialog", { name: "Upload a doc" });
    expect(within(dialog).getByText("Drop a Markdown file here")).toBeInTheDocument();
    expect(within(dialog).getByText("Markdown (.md) only, up to 2 MB")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Choose a file" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Upload doc" })).toHaveAttribute("aria-disabled", "true");
    expect(chooser()).toHaveAttribute("accept", ".md,text/markdown");
  });

  it("names a file that is not Markdown and the next step, and sends nothing", async () => {
    mount({ kind: "create" });
    await user().upload(chooser(), md("%PDF", "retail-notes.pdf"));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("retail-notes.pdf");
    expect(alert).toHaveTextContent("Only Markdown (.md) files are accepted.");
    expect(alert).toHaveTextContent("Export the document as Markdown and choose it again.");
    expect(screen.getByRole("button", { name: "Upload doc" })).toHaveAttribute("aria-disabled", "true");
    await user().click(screen.getByRole("button", { name: "Upload doc" }));
    expect(sent).toEqual([]);
  });

  it("names a file over 2 MB", async () => {
    mount({ kind: "create" });
    const big = md("x", "export-all-flags.md");
    Object.defineProperty(big, "size", { value: 3.4 * 1024 * 1024 });
    fireEvent.change(chooser(), { target: { files: [big] } });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("File is over the 2 MB limit.");
    expect(alert).toHaveTextContent("3.4 MB");
    expect(alert).toHaveTextContent("Split it into smaller docs");
  });

  it("takes a file that is dropped on the zone", async () => {
    mount({ kind: "create" });
    fireEvent.drop(screen.getByTestId("drop-zone"), { dataTransfer: { files: [md("# Hi")] } });
    expect(await screen.findByDisplayValue("Instant payments")).toBeInTheDocument();
    expect(screen.getByText("Drop a different file")).toBeInTheDocument();
  });

  it("shows a ready file with its title, and says what the button will do before it is pressed", async () => {
    mount({ kind: "create" });
    await user().upload(chooser(), md("# Instant payments\n"));
    const ready = await screen.findByText("Ready to upload.");
    expect(ready.closest("[role=status]")).toHaveTextContent("instant-payments.md");
    expect(screen.getByLabelText("Title")).toHaveValue("Instant payments");
    expect(screen.getByRole("alertdialog")).toHaveTextContent("Adds a new doc");
    expect(screen.getByRole("button", { name: "Upload doc" })).not.toHaveAttribute("aria-disabled");
  });

  it("sends the title and the text, with a CSRF token and an Idempotency-Key, and hands the new doc on", async () => {
    answer = () => ok(UPLOAD_DOC, 201);
    const { onDone, onOpenChange } = mount({ kind: "create" });
    await user().upload(chooser(), md("# Instant payments\n\nText.\n"));
    const title = await screen.findByLabelText("Title");
    await user().clear(title);
    await user().type(title, "Instant payments (rail)");
    await user().click(screen.getByRole("button", { name: "Upload doc" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith(UPLOAD_DOC));
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({ method: "POST", path: "/api/v1/docs", body: { title: "Instant payments (rail)", markdown: "# Instant payments\n\nText.\n" } });
    expect(sent[0]?.headers.get("x-csrf-token")).toBe("AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde");
    expect(sent[0]?.headers.get("idempotency-key")).toMatch(/^[A-Za-z0-9._-]{1,128}$/);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("will not send a doc with no title", async () => {
    mount({ kind: "create" });
    await user().upload(chooser(), md("# x"));
    await user().clear(await screen.findByLabelText("Title"));
    expect(screen.getByRole("button", { name: "Upload doc" })).toHaveAttribute("aria-disabled", "true");
    await user().click(screen.getByRole("button", { name: "Upload doc" }));
    expect(sent).toEqual([]);
  });

  it("stays open with the file and the title when the server refuses, and says what happened with the request id", async () => {
    answer = () => fail(409, "conflict", "Conflict");
    const { onDone, onOpenChange } = mount({ kind: "create" });
    await user().upload(chooser(), md("# x"));
    await user().type(await screen.findByLabelText("Title"), " two");
    await user().click(screen.getByRole("button", { name: "Upload doc" }));
    const alerts = await screen.findAllByRole("alert");
    const error = alerts.find((a) => a.textContent?.includes("req-doc-1"));
    expect(error).toHaveTextContent("A doc with this name already exists");
    expect(error).toHaveTextContent("Nothing was saved");
    expect(screen.getByLabelText("Title")).toHaveValue("Instant payments two");
    expect(screen.getByText("instant-payments.md")).toBeInTheDocument();
    expect(onDone).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it.each([
    [413, "payload_too_large", "The doc is too large"],
    [403, "forbidden", "You do not have the Editor role"],
    [400, "bad_request", "The doc was not accepted"],
    [503, "unavailable", "The server could not save the doc"],
  ])("explains a %i", async (status, type, text) => {
    answer = () => fail(status, type, "Whatever the server called it");
    mount({ kind: "create" });
    await user().upload(chooser(), md("# x"));
    await user().click(await screen.findByRole("button", { name: "Upload doc" }));
    const error = (await screen.findAllByRole("alert")).find((a) => a.textContent?.includes("req-doc-1"));
    expect(error).toHaveTextContent(text);
    expect(error).not.toHaveTextContent("Whatever the server called it");
  });

  it("locks the dialog while the write runs", async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        await held;
        return ok(UPLOAD_DOC, 201);
      }),
    );
    mount({ kind: "create" });
    await user().upload(chooser(), md("# x"));
    await user().click(await screen.findByRole("button", { name: "Upload doc" }));
    expect(await screen.findByRole("button", { name: "Uploading…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    release();
  });
});

describe("replace", () => {
  it("says what it replaces before anything else, and has no title to type", () => {
    mount({ kind: "replace", doc: UPLOAD_DOC });
    const dialog = screen.getByRole("alertdialog", { name: "Replace “Instant payments”" });
    expect(dialog).toHaveTextContent("Replaces the current text of this doc, version 3, saved 2 d ago.");
    expect(screen.queryByLabelText("Title")).toBeNull();
    expect(within(dialog).getByRole("button", { name: "Replace doc" })).toHaveAttribute("aria-disabled", "true");
  });

  it("lists the consequences once a file is ready", async () => {
    mount({ kind: "replace", doc: UPLOAD_DOC });
    await user().upload(chooser(), md("# Instant payments\n"));
    await screen.findByText("Ready to replace.");
    const dialog = screen.getByRole("alertdialog");
    expect(dialog).toHaveTextContent("Saves as version 4.");
    expect(dialog).toHaveTextContent("see the new text");
    expect(dialog).toHaveTextContent("may stop working");
  });

  it("sends the text with the version it was made against", async () => {
    const { onDone } = mount({ kind: "replace", doc: UPLOAD_DOC });
    await user().upload(chooser(), md("# New\n"));
    await user().click(await screen.findByRole("button", { name: "Replace doc" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledWith({ ...UPLOAD_DOC, version: 4 }));
    expect(sent[0]).toMatchObject({ method: "PUT", path: "/api/v1/docs/102", body: { markdown: "# New\n", expected_version: 3 } });
    expect(sent[0]?.headers.get("idempotency-key")).toMatch(/^[A-Za-z0-9._-]{1,128}$/);
  });

  it("says someone else got there first on a 409, and keeps the file so that it can be sent again", async () => {
    answer = () => fail(409, "conflict", "Conflict");
    const { onDone } = mount({ kind: "replace", doc: UPLOAD_DOC });
    await user().upload(chooser(), md("# New\n"));
    await user().click(await screen.findByRole("button", { name: "Replace doc" }));
    const error = (await screen.findAllByRole("alert")).find((a) => a.textContent?.includes("req-doc-1"));
    expect(error).toHaveTextContent("Someone else changed it first");
    expect(error).toHaveTextContent("Nothing was saved");
    expect(screen.getByText("instant-payments.md")).toBeInTheDocument();
    expect(onDone).not.toHaveBeenCalled();
  });

  it("says the doc is gone on a 404", async () => {
    answer = () => fail(404, "not_found", "Not found");
    mount({ kind: "replace", doc: UPLOAD_DOC });
    await user().upload(chooser(), md("# New\n"));
    await user().click(await screen.findByRole("button", { name: "Replace doc" }));
    const error = (await screen.findAllByRole("alert")).find((a) => a.textContent?.includes("req-doc-1"));
    expect(error).toHaveTextContent("This doc no longer exists");
  });

  it("forgets the file when the dialog is opened again", async () => {
    client = createQueryClient();
    const view = render(
      <QueryClientProvider client={client}>
        <UploadDocDialog open onOpenChange={() => undefined} mode={{ kind: "replace", doc: UPLOAD_DOC }} />
      </QueryClientProvider>,
    );
    await user().upload(chooser(), md("# New\n"));
    await screen.findByText("Ready to replace.");
    view.rerender(
      <QueryClientProvider client={client}>
        <UploadDocDialog open={false} onOpenChange={() => undefined} mode={{ kind: "replace", doc: UPLOAD_DOC }} />
      </QueryClientProvider>,
    );
    view.rerender(
      <QueryClientProvider client={client}>
        <UploadDocDialog open onOpenChange={() => undefined} mode={{ kind: "replace", doc: UPLOAD_DOC }} />
      </QueryClientProvider>,
    );
    expect(screen.queryByText("Ready to replace.")).toBeNull();
    expect(screen.getByText("Drop a Markdown file here")).toBeInTheDocument();
  });
});
