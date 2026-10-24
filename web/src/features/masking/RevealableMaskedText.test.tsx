import type { QueryClient } from "@tanstack/react-query";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import type { Mask } from "@/api/generated/model";
import { toasts } from "@/features/shell/toast/store";
import { renderFile, routes, json, problem, type Api, type FileHarnessOptions } from "@/features/file/test-utils";
import { expectNoAxeViolations } from "@/test/axe";
import { REVEAL_VISIBLE_MS, MAX_SHOWN_CHARS } from "./use-reveal";
import { RevealableMaskedText } from "./RevealableMaskedText";
import { expectNowhere, maskOf as baseMaskOf, refusal, utf8, watchConsole } from "./test-utils";

// The Reveal of a masked value (S14b, D79; the handoff's File page, frames 1d, 1e and 1f). It is the one way the page ever holds a
// value the server hid, so the tests are about where the value is: only in the one revealed span, only for as long as it is shown, and
// nowhere else (not an attribute, a name, the query cache, the mutation cache, storage, the console, the toasts or the clipboard)
// after success, after every failure, after the time is up, when the person hides it or leaves, and when the component goes.

const SWIMLANE = "sitb";
const PATH = "tx-infinity-api/tx-infinity-core-sit7.yml";
const SETTING = "txInfinityOptions.remote-itm-teller.clientSecret";
const SECRET = "tellerClient-9f2c71";
const MARK = "[REDACTED: 19 chars, generic_secret]";
const TEXT = `service:\n  name: core\n    clientSecret: ${MARK}\nport: 8080\n`;
const bytes = utf8;
const maskOf = (text: string, line: number, extra: Partial<Mask> = {}): Mask => baseMaskOf(text, line, MARK, extra);
const MASK = maskOf(TEXT, 3, { setting: SETTING });
const REASON = "Teller login fails on sit7 (OPS-2291)";
const REVEAL = `POST /api/v1/swimlanes/${SWIMLANE}/files/reveal`;
const REVEAL_BUTTON = `Reveal value of ${SETTING}`;

type Props = Partial<React.ComponentProps<typeof RevealableMaskedText>>;
function view(props: Props = {}, options: FileHarnessOptions = {}, answer: () => Response | Promise<Response> = () => json({ value: SECRET })) {
  // A handler that returns a promise: `routes` would turn the promise into a JSON body, so the answer is chosen here.
  const api: Api = options.api ?? ((call) => (`${call.method} ${call.path}` === REVEAL ? answer() : problem(404, "not_found")));
  return renderFile(<RevealableMaskedText swimlane={SWIMLANE} path={PATH} label="core.yml" text={TEXT} masks={[MASK]} {...props} />, { ...options, api });
}

const expectSecretNowhere = (client: QueryClient, logged: unknown[][], options: { except?: "the revealed span" } = {}) => {
  expectNowhere(SECRET, client, logged, { inRevealedSpan: options.except === "the revealed span" });
  expect(Object.entries(localStorage).join() + Object.entries(sessionStorage).join(), "web storage").not.toContain(SECRET);
};

let user: ReturnType<typeof userEvent.setup>;
let writes: MockInstance<Clipboard["writeText"]>;
beforeEach(() => {
  toasts.clear();
  user = userEvent.setup({ delay: null });
  writes = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue(undefined);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function openDialog(): Promise<HTMLElement> {
  await user.click(await screen.findByRole("button", { name: REVEAL_BUTTON }));
  return screen.findByRole("alertdialog", { name: "Reveal masked value?" });
}
async function reveal(reason = REASON): Promise<HTMLElement> {
  const dialog = await openDialog();
  await user.type(within(dialog).getByLabelText(/Reason/), reason);
  await user.click(within(dialog).getByRole("button", { name: "Reveal value" }));
  return dialog;
}
const revealCalls = <T extends { method: string; path: string }>(calls: T[]): T[] => calls.filter((c) => `${c.method} ${c.path}` === REVEAL);
/** Move the fake clock, and let what that sets going settle. */
const advance = (ms: number) =>
  act(async () => {
    vi.advanceTimersByTime(ms);
    await Promise.resolve();
  });

describe("the control", () => {
  it("draws the token with a Reveal button beside it, named for the setting, and no value", async () => {
    view();
    expect(await screen.findByRole("button", { name: REVEAL_BUTTON })).toBeInTheDocument();
    const row = screen.getAllByRole("row")[2] as HTMLElement;
    expect(row.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    expect(row).toHaveTextContent("clientSecret:");
    expect(row.textContent).not.toContain("REDACTED");
  });

  it("offers no Reveal for a marker with no setting (plain text, served text, a doc), only the token", async () => {
    view({ masks: [maskOf(TEXT, 3)] });
    await screen.findAllByRole("row");
    expect(document.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    expect(screen.queryByRole("button", { name: /Reveal/ })).toBeNull();
  });

  it("disables Reveal for a Viewer, says which role it needs, and asks for nothing when pressed", async () => {
    const { calls } = view({}, { role: "viewer" });
    const button = await screen.findByRole("button", { name: REVEAL_BUTTON });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription(/Needs the Editor role/);
    await user.click(button);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(revealCalls(calls)).toHaveLength(0);
  });

  it("is off while the live connection is down, like every audited action", async () => {
    view({}, { online: false });
    expect(await screen.findByRole("button", { name: REVEAL_BUTTON })).toHaveAccessibleDescription("Unavailable offline");
  });
});

describe("the consequence dialog", () => {
  it("says the reveal is audited, names the setting, and asks for a reason", async () => {
    view();
    const dialog = await openDialog();
    expect(within(dialog).getByText("Reveal masked value?")).toBeInTheDocument();
    expect(dialog).toHaveTextContent(SETTING);
    expect(dialog).toHaveTextContent(/recorded in the audit log with your name, the setting and your reason/i);
    expect(dialog).toHaveTextContent(/value is not stored in the log/i);
    expect(within(dialog).getByLabelText(/Reason/)).toBeRequired();
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  });

  it("moves focus to the reason, and has no axe violations", async () => {
    view();
    const dialog = await openDialog();
    await waitFor(() => expect(within(dialog).getByLabelText(/Reason/)).toHaveFocus());
    await expectNoAxeViolations(dialog);
  });

  it("will not send without a reason: it says so, and no request is made", async () => {
    const { calls } = view();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("button", { name: "Reveal value" }));
    expect(await within(dialog).findByText("Enter a reason")).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/Reason/)).toHaveAttribute("aria-invalid", "true");
    await user.type(within(dialog).getByLabelText(/Reason/), "   ");
    await user.click(within(dialog).getByRole("button", { name: "Reveal value" }));
    expect(revealCalls(calls)).toHaveLength(0);
  });

  it("cancels without a request and gives focus back to the Reveal button", async () => {
    const { calls } = view();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(revealCalls(calls)).toHaveLength(0);
    expect(screen.getByRole("button", { name: REVEAL_BUTTON })).toHaveFocus();
  });
});

describe("a reveal that works", () => {
  it("sends the file, the setting and the reason once, with the CSRF token and a fresh Idempotency-Key", async () => {
    const { calls } = view();
    await reveal();
    await screen.findByText(SECRET);
    const sent = revealCalls(calls);
    expect(sent).toHaveLength(1);
    expect(JSON.parse(sent[0]?.body ?? "{}")).toEqual({ path: PATH, setting: SETTING, reason: REASON });
    expect(sent[0]?.headers.get("X-CSRF-Token")).toBeTruthy();
    expect(sent[0]?.headers.get("Idempotency-Key")).toBeTruthy();
  });

  it("shows the value in place of the token, with Hide and the audit note, closes the dialog, and moves focus to Hide", async () => {
    view();
    await reveal();
    const value = await screen.findByText(SECRET);
    expect(value.closest("[data-slot='revealed-value']")).not.toBeNull();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    const row = screen.getAllByRole("row")[2] as HTMLElement;
    expect(row).toHaveTextContent("Revealed · recorded in the audit log");
    expect(row.querySelector("[data-slot='hidden-value']")).toBeNull();
    const hide = within(row).getByRole("button", { name: `Hide value of ${SETTING}` });
    await waitFor(() => expect(hide).toHaveFocus());
  });

  it("makes room for a value longer than the marker: the line is as wide as the value, Hide and the note, and does not clip them", async () => {
    const long = "k9".repeat(60);
    view({}, {}, () => json({ value: long }));
    const spacer = (): string => (document.querySelector("[data-slot='virtual-spacer']") as HTMLElement).style.minWidth;
    await screen.findByRole("button", { name: REVEAL_BUTTON });
    const before = spacer();
    await reveal();
    await screen.findByText(long);
    const chars = (text: string): number => Number(/calc\((\d+)ch/.exec(text)?.[1] ?? "0");
    expect(chars(spacer()) - chars(before), "the view is wider by the value and the words after it").toBeGreaterThanOrEqual(long.length);
    await user.click(screen.getByRole("button", { name: `Hide value of ${SETTING}` }));
    await waitFor(() => expect(chars(spacer())).toBe(chars(before)));
  });

  it("keeps the value in that one span and nowhere else, and never copies it", async () => {
    const logged = watchConsole();
    const { client } = view();
    await reveal();
    await screen.findByText(SECRET);
    expectSecretNowhere(client, logged, { except: "the revealed span" });
    expect(screen.queryByRole("button", { name: /copy/i })).toBeNull();
    // Every button on the page, pressed: none writes the clipboard.
    for (const button of screen.getAllByRole("button")) await user.click(button);
    expect(writes).not.toHaveBeenCalled();
  });

  it("announces the reveal politely, without the value", async () => {
    view();
    await reveal();
    await screen.findByText(SECRET);
    const live = document.querySelector("[aria-live='polite']");
    expect(live?.textContent).toMatch(/Value revealed/);
    expect(live?.textContent).not.toContain(SECRET);
  });

  it("hides the value when the person presses Hide, and puts focus back on Reveal", async () => {
    const logged = watchConsole();
    const { client } = view();
    await reveal();
    await user.click(await screen.findByRole("button", { name: `Hide value of ${SETTING}` }));
    await waitFor(() => expect(screen.queryByText(SECRET)).toBeNull());
    expectSecretNowhere(client, logged);
    expect(screen.getByRole("button", { name: REVEAL_BUTTON })).toHaveFocus();
    expect(document.querySelector("[aria-live='polite']")?.textContent).toMatch(/Value hidden/);
  });

  it("hides the value on its own after the time is up", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const logged = watchConsole();
    const { client } = view();
    await reveal();
    await screen.findByText(SECRET);
    await advance(REVEAL_VISIBLE_MS - 1_000);
    expect(screen.getByText(SECRET)).toBeInTheDocument();
    await advance(1_500);
    expect(screen.queryByText(SECRET)).toBeNull();
    expectSecretNowhere(client, logged);
    expect(screen.getByRole("button", { name: REVEAL_BUTTON })).toBeInTheDocument();
  });

  it("hides the value when the window loses focus, or the tab is hidden", async () => {
    const logged = watchConsole();
    const { client } = view();
    await reveal();
    await screen.findByText(SECRET);
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    expect(screen.queryByText(SECRET)).toBeNull();
    expectSecretNowhere(client, logged);

    await user.click(await screen.findByRole("button", { name: REVEAL_BUTTON }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByLabelText(/Reason/), REASON);
    await user.click(within(dialog).getByRole("button", { name: "Reveal value" }));
    await screen.findByText(SECRET);
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    Reflect.deleteProperty(document, "visibilityState");
    expect(screen.queryByText(SECRET)).toBeNull();
    expectSecretNowhere(client, logged);
  });

  it("does not hide the value for a visibility change to visible", async () => {
    view();
    await reveal();
    await screen.findByText(SECRET);
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(screen.getByText(SECRET)).toBeInTheDocument();
  });

  it("shows one value at a time: revealing another hides the first", async () => {
    const text = `a: ${MARK}\nb: ${MARK}\n`;
    const first = maskOf(text, 1, { setting: "a" });
    const secondStart = bytes(text.slice(0, text.lastIndexOf(MARK)));
    const second: Mask = { line: 2, start: secondStart, end: secondStart + bytes(MARK), setting: "b" };
    let n = 0;
    view({ text, masks: [first, second] }, {}, () => json({ value: n++ === 0 ? SECRET : "second-value-1234" }));
    await user.click(await screen.findByRole("button", { name: "Reveal value of a" }));
    await user.type(within(await screen.findByRole("alertdialog")).getByLabelText(/Reason/), REASON);
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Reveal value" }));
    await screen.findByText(SECRET);
    await user.click(await screen.findByRole("button", { name: "Reveal value of b" }));
    await user.type(within(await screen.findByRole("alertdialog")).getByLabelText(/Reason/), REASON);
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Reveal value" }));
    await screen.findByText("second-value-1234");
    expect(screen.queryByText(SECRET)).toBeNull();
  });

  /** A host whose text and file the test can change from the page, the way the file view's own data changes under it. */
  function Host() {
    const [where, setWhere] = useState({ path: PATH, text: TEXT });
    return (
      <>
        <button type="button" onClick={() => setWhere({ path: PATH, text: `${TEXT}extra: 1\n` })}>
          Change the text
        </button>
        <button type="button" onClick={() => setWhere({ path: "other/file.yml", text: TEXT })}>
          Change the file
        </button>
        <RevealableMaskedText swimlane={SWIMLANE} path={where.path} label="core.yml" text={where.text} masks={[MASK]} />
      </>
    );
  }

  it("clears the value when the text it was revealed in changes", async () => {
    const logged = watchConsole();
    const harness = renderFile(<Host />, { api: (call) => (`${call.method} ${call.path}` === REVEAL ? json({ value: SECRET }) : problem(404, "not_found")) });
    await reveal();
    await screen.findByText(SECRET);
    await user.click(screen.getByRole("button", { name: "Change the text" }));
    expect(screen.queryByText(SECRET)).toBeNull();
    expectSecretNowhere(harness.client, logged);
  });

  it("clears the value when the file changes", async () => {
    const logged = watchConsole();
    const harness = renderFile(<Host />, { api: (call) => (`${call.method} ${call.path}` === REVEAL ? json({ value: SECRET }) : problem(404, "not_found")) });
    await reveal();
    await screen.findByText(SECRET);
    await user.click(screen.getByRole("button", { name: "Change the file" }));
    expect(screen.queryByText(SECRET)).toBeNull();
    expectSecretNowhere(harness.client, logged);
  });

  it("drops an answer that comes back for a text that has since changed", async () => {
    const logged = watchConsole();
    let release: (response: Response) => void = () => undefined;
    const pending = new Promise<Response>((resolve) => (release = resolve));
    const harness = renderFile(<Host />, { api: (call) => (`${call.method} ${call.path}` === REVEAL ? pending : problem(404, "not_found")) });
    await reveal();
    await waitFor(() => expect(revealCalls(harness.calls)).toHaveLength(1));
    // The dialog is modal while the request is out; the text changes under it, the way a live update would change it.
    fireEvent.click(screen.getByText("Change the text"));
    await act(async () => {
      release(json({ value: SECRET }));
      await pending;
    });
    expect(screen.queryByText(SECRET)).toBeNull();
    expectSecretNowhere(harness.client, logged);
  });

  it("works under StrictMode, which runs every effect twice", async () => {
    const api = routes({ [REVEAL]: () => json({ value: SECRET }) });
    const harness = renderFile(
      <StrictMode>
        <RevealableMaskedText swimlane={SWIMLANE} path={PATH} label="core.yml" text={TEXT} masks={[MASK]} />
      </StrictMode>,
      { api },
    );
    await reveal();
    expect(await screen.findByText(SECRET)).toBeInTheDocument();
    harness.unmount();
    expectSecretNowhere(harness.client, []);
  });

  it("clips a very long value, shows hidden characters in it as markers, and says it was clipped", async () => {
    const long = `${"A".repeat(MAX_SHOWN_CHARS)}TAIL_NOT_SHOWN`;
    view({}, {}, () => json({ value: `x\u202ey${long}` }));
    await reveal();
    const span = await waitFor(() => {
      const found = document.querySelector("[data-slot='revealed-value']");
      expect(found).not.toBeNull();
      return found as HTMLElement;
    });
    expect(span.textContent).not.toContain("TAIL_NOT_SHOWN");
    expect(span).toHaveTextContent("<U+202E>");
    expect(span).toHaveTextContent(`first ${MAX_SHOWN_CHARS.toLocaleString("en-US")} characters`);
  });

  it("has no axe violations while a value is shown", async () => {
    const { container } = view();
    await reveal();
    await screen.findByText(SECRET);
    await expectNoAxeViolations(container);
  });
});

describe("when the component goes", () => {
  it("leaves nothing behind when it unmounts with the value shown", async () => {
    const logged = watchConsole();
    const harness = view();
    await reveal();
    await screen.findByText(SECRET);
    harness.unmount();
    expectSecretNowhere(harness.client, logged);
    expect(harness.client.getMutationCache().getAll()).toHaveLength(0);
  });

  it("drops an answer that arrives after it went: nothing is shown, nothing is kept, nothing is logged", async () => {
    const logged = watchConsole();
    let release: (response: Response) => void = () => undefined;
    const pending = new Promise<Response>((resolve) => (release = resolve));
    const harness = view({}, {}, () => pending);
    await reveal();
    await waitFor(() => expect(revealCalls(harness.calls)).toHaveLength(1));
    harness.unmount();
    await act(async () => {
      release(json({ value: SECRET }));
      await pending;
    });
    expect(document.body.innerHTML).not.toContain(SECRET);
    expectSecretNowhere(harness.client, logged);
    expect(harness.client.getMutationCache().getAll()).toHaveLength(0);
  });
});

describe("a reveal that fails", () => {
  async function failsWith(answer: () => Response | Promise<Response>, expected: RegExp, options: FileHarnessOptions = {}) {
    const logged = watchConsole();
    const harness = view({}, options, answer);
    const dialog = await reveal();
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(expected);
    // The dialog stays open with the reason the person wrote; nothing was shown or kept.
    expect(within(dialog).getByLabelText(/Reason/)).toHaveValue(REASON);
    expect(screen.queryByText(SECRET)).toBeNull();
    expectSecretNowhere(harness.client, logged);
    return { harness, dialog };
  }

  it("429: says how long to wait, once, and never asks again by itself", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { harness } = await failsWith(() => refusal(429, "too_many_requests", { "retry-after": "37" }), /Wait 37 seconds/);
    expect(screen.getByText("req-test-1")).toBeInTheDocument();
    await advance(120_000);
    expect(revealCalls(harness.calls)).toHaveLength(1);
  });

  it("503: says the audit log did not take the record and that nothing was shown", async () => {
    const response = () => refusal(503, "service_unavailable", { "retry-after": "5" });
    const { harness } = await failsWith(response, /audit log could not take the record/);
    expect(revealCalls(harness.calls)).toHaveLength(1);
  });

  it("403: says the role does not allow it", async () => {
    await failsWith(() => problem(403, "forbidden"), /role/i);
  });

  it("404: says there is nothing to reveal there", async () => {
    await failsWith(() => problem(404, "not_found"), /nothing to reveal/i);
  });

  it("500: says a reveal may have been recorded, and that trying again records another", async () => {
    const { harness } = await failsWith(() => problem(500, "internal"), /may have recorded a reveal that you did not see/);
    expect(revealCalls(harness.calls)).toHaveLength(1);
  });

  it("a lost connection: says a reveal may have been recorded, and does not retry", async () => {
    const calls: number[] = [];
    const logged = watchConsole();
    const harness = view({}, {}, () => {
      calls.push(1);
      throw new TypeError("Failed to fetch");
    });
    const dialog = await reveal();
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/may have recorded/);
    // The fetch layer retries other writes once after a network error; a reveal is never retried (a retry is a second audited reveal).
    await new Promise((resolve) => setTimeout(resolve, 600));
    expect(calls).toHaveLength(1);
    expectSecretNowhere(harness.client, logged);
  });

  it("a timeout: says a reveal may have been recorded, and does not retry", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const logged = watchConsole();
    let calls = 0;
    const harness = view();
    vi.stubGlobal(
      "fetch",
      vi.fn((_url: string, init: RequestInit) => {
        calls += 1;
        return new Promise<Response>((_resolve, reject) => init.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError"))));
      }),
    );
    const dialog = await reveal();
    await advance(16_000);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/may have recorded/);
    await advance(120_000);
    expect(calls).toBe(1);
    expectSecretNowhere(harness.client, logged);
  });

  it("sends again only when the person presses the button again, as a new request with a new key", async () => {
    let n = 0;
    const { harness, dialog } = await failsWith(() => (n++ === 0 ? problem(500, "internal") : json({ value: SECRET })), /may have recorded/);
    await user.click(within(dialog).getByRole("button", { name: "Reveal value" }));
    await screen.findByText(SECRET);
    const sent = revealCalls(harness.calls);
    expect(sent).toHaveLength(2);
    expect(sent[0]?.headers.get("Idempotency-Key")).not.toBe(sent[1]?.headers.get("Idempotency-Key"));
  });

  it("ignores a second press while the first is in flight", async () => {
    let release: (response: Response) => void = () => undefined;
    const pending = new Promise<Response>((resolve) => (release = resolve));
    const harness = view({}, {}, () => pending);
    const dialog = await reveal();
    const button = within(dialog).getByRole("button", { name: /Reveal value|Revealing/ });
    await user.click(button);
    await user.keyboard("{Enter}");
    expect(revealCalls(harness.calls)).toHaveLength(1);
    await act(async () => {
      release(json({ value: SECRET }));
      await pending;
    });
    await screen.findByText(SECRET);
    expect(revealCalls(harness.calls)).toHaveLength(1);
  });

  it("has no axe violations with the error shown", async () => {
    const { dialog } = await failsWith(() => problem(404, "not_found"), /nothing to reveal/i);
    await expectNoAxeViolations(dialog);
  });
});
