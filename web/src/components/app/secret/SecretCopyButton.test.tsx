import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef, useState, type ComponentProps } from "react";
import { afterEach, beforeEach, describe, expect, expectTypeOf, it, vi } from "vitest";
import { toasts } from "@/features/shell/toast/store";
import { SecretCopyButton } from "./SecretCopyButton";
import { SecretField } from "./SecretField";

// The copy button of a one-time secret (the admin join token, a personal MCP token). Stricter than CopyButton: a click is the
// only way it writes, the value comes from a getter at that moment and is kept nowhere, the words it says never hold the value,
// and it leaves nothing behind when the secret is put away. These tests scan the DOM, the caches, the console and the toaster.

const SECRET = "lkpat_ZxY9w8V7u6T5s4R3q2P1o0NmLkJiHgFeDcBaZxY9w8V7u6T5s4R3q2P1o0N";
const SAID = "Copied. It will not be shown again.";
const FAILED = "Couldn't copy. Select the text and copy it manually.";

let client: QueryClient;
let calls: { get: number };
const consoleSpies = () => (["log", "info", "warn", "error", "debug", "trace"] as const).map((level) => vi.spyOn(console, level).mockImplementation(() => undefined));

function Harness({ onStored }: { onStored?: () => void }) {
  const field = useRef<HTMLInputElement>(null);
  const [issued, setIssued] = useState<string | null>(SECRET);
  if (issued === null) return null;
  return (
    <div>
      <SecretField ref={field} aria-label="Your token" value={issued} />
      <SecretCopyButton
        what="token"
        source={field}
        getValue={() => {
          calls.get += 1;
          return issued;
        }}
      />
      <button
        type="button"
        onClick={() => {
          setIssued(null);
          onStored?.();
        }}
      >
        I have stored it
      </button>
    </div>
  );
}

function mount(ui = <Harness />) {
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

/** Every place in the page that holds the secret, other than the one read-only field that shows it. */
function exposures(root: ParentNode = document.body): string[] {
  const found: string[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    if (node.nodeType === Node.TEXT_NODE) {
      if ((node.textContent ?? "").includes(SECRET)) found.push(`text in <${node.parentElement?.tagName.toLowerCase() ?? "?"}>`);
      continue;
    }
    const element = node as Element;
    const isField = element instanceof HTMLInputElement && element.getAttribute("aria-label") === "Your token";
    for (const attribute of Array.from(element.attributes)) {
      if (isField && attribute.name === "value") continue;
      if (attribute.value.includes(SECRET)) found.push(`${element.tagName.toLowerCase()}[${attribute.name}]`);
    }
  }
  return found;
}

const cachesHold = (): string => JSON.stringify([client.getQueryCache().getAll().map((q) => [q.queryKey, q.state.data]), client.getMutationCache().getAll().map((m) => [m.options.mutationKey, m.state.variables, m.state.data, m.state.error])]);

beforeEach(() => {
  client = new QueryClient();
  calls = { get: 0 };
  toasts.clear();
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "userActivation");
  window.getSelection()?.removeAllRanges();
});

describe("SecretCopyButton, when it copies", () => {
  it("is named for the secret, and nothing is read or written until the person clicks", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    const view = mount();
    const button = screen.getByRole("button", { name: "Copy token" });
    view.rerender(
      <QueryClientProvider client={client}>
        <Harness />
      </QueryClientProvider>,
    );
    await user.hover(button);
    await user.tab();
    await user.tab();
    expect(calls.get).toBe(0);
    expect(write).not.toHaveBeenCalled();
    await user.click(button);
    expect(calls.get).toBe(1);
    expect(write).toHaveBeenCalledTimes(1);
    expect(write).toHaveBeenCalledWith(SECRET);
  });

  it("copies the value the getter gives at the moment of the click, not one captured earlier", async () => {
    const user = userEvent.setup();
    let current = "first-value-aaaaaaaaaaaaaaaaaaaaaaaa";
    render(<SecretCopyButton what="token" getValue={() => current} />);
    current = "second-value-bbbbbbbbbbbbbbbbbbbbbbb";
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(await navigator.clipboard.readText()).toBe("second-value-bbbbbbbbbbbbbbbbbbbbbbb");
  });

  it("says Copied with a check, and says to the screen reader that it will not be shown again, without the value", async () => {
    const user = userEvent.setup();
    mount();
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(screen.getByRole("status")).toHaveTextContent(SAID);
    expect(screen.getByRole("status")).toHaveClass("sr-only");
    const button = screen.getByRole("button", { name: "Copy token" });
    expect(within(button).getByText("Copied")).not.toHaveAttribute("aria-hidden");
    expect(within(button).getByText("Copy token")).toHaveAttribute("aria-hidden", "true");
    expect(button.querySelector('[data-slot="copy-check"]')).not.toBeNull();
    // The name did not change under the person's focus, and the button took no focus from them.
    expect(button).toHaveFocus();
    expect(button).toBeEnabled();
  });

  it("never holds the value in an accessible name, a title, a data attribute, a key or text: only the field that shows it does", async () => {
    const user = userEvent.setup();
    mount();
    expect(exposures()).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(exposures()).toEqual([]);
    expect(document.querySelectorAll(`[value="${SECRET}"]`)).toHaveLength(1);
    expect(screen.getByLabelText("Your token")).toHaveValue(SECRET);
  });

  it("is a labelled button, not an icon alone, so its words stay put when it says Copied", async () => {
    const user = userEvent.setup();
    render(<SecretCopyButton what="join token" getValue={() => SECRET} />);
    const button = screen.getByRole("button", { name: "Copy join token" });
    expect(button).toHaveTextContent("Copy join token");
    await user.click(button);
    expect(button).toHaveTextContent("Copied");
    expect(button).toHaveTextContent("Copy join token");
  });

  it("goes back to Copy after 1.5 s, and the announcement empties", async () => {
    userEvent.setup();
    vi.useFakeTimers();
    mount();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy token" }));
      await Promise.resolve();
    });
    expect(screen.getByRole("status")).toHaveTextContent(SAID);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1600);
    });
    expect(screen.getByRole("status")).toHaveTextContent("");
    const button = screen.getByRole("button", { name: "Copy token" });
    expect(within(button).getByText("Copied")).toHaveAttribute("aria-hidden", "true");
    expect(within(button).getByText("Copy token")).not.toHaveAttribute("aria-hidden");
  });
});

describe("SecretCopyButton, what it will not do", () => {
  it("refuses an empty value and a redaction marker: a masked value has no clipboard text", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    const { unmount } = render(<SecretCopyButton what="token" getValue={() => ""} />);
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(screen.getByRole("status")).toHaveTextContent("Nothing to copy.");
    unmount();
    render(<SecretCopyButton what="token" getValue={() => "[REDACTED: 40 chars, github-pat]"} />);
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(screen.getByRole("status")).toHaveTextContent("Nothing to copy.");
    expect(write).not.toHaveBeenCalled();
  });

  it("refuses a value with a line break or control character, as the generic button does", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    render(<SecretCopyButton what="token" getValue={() => "abc\nrm -rf ~"} />);
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(write).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("Not copied");
  });

  it("writes only for a click the person made: with no user activation the browser would refuse, so it does not try", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    Object.defineProperty(navigator, "userActivation", { configurable: true, value: { isActive: false, hasBeenActive: false } });
    mount();
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(write).not.toHaveBeenCalled();
    expect(calls.get).toBe(0);
    expect(screen.getByRole("status")).toHaveTextContent(FAILED);
  });

  it("has no way to be given the value as a prop", () => {
    expectTypeOf<ComponentProps<typeof SecretCopyButton>>().not.toHaveProperty("value");
    expectTypeOf<ComponentProps<typeof SecretCopyButton>["getValue"]>().toEqualTypeOf<() => string>();
  });
});

describe("SecretCopyButton, when the clipboard says no", () => {
  it("says so, selects the whole token in the field so Control+C works, and the message holds no value", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValueOnce(new Error(`denied ${SECRET}`));
    const spies = consoleSpies();
    mount();
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(screen.getByRole("status")).toHaveTextContent(FAILED);
    const field = screen.getByLabelText<HTMLInputElement>("Your token");
    expect(field).toHaveFocus();
    expect(field.selectionStart).toBe(0);
    expect(field.selectionEnd).toBe(SECRET.length);
    expect(exposures()).toEqual([]);
    for (const spy of spies) expect(spy).not.toHaveBeenCalled();
  });

  it("says so when the page has no Clipboard API", async () => {
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
    mount();
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(screen.getByRole("status")).toHaveTextContent(FAILED);
  });
});

describe("SecretCopyButton, what it leaves behind", () => {
  it("leaves nothing in the query cache, the mutation cache, the console or the toaster after a copy", async () => {
    const user = userEvent.setup();
    const spies = consoleSpies();
    mount();
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    expect(cachesHold()).not.toContain(SECRET);
    expect(toasts.getSnapshot()).toEqual([]);
    for (const spy of spies) expect(spy).not.toHaveBeenCalled();
    expect(screen.queryByRole("region", { name: "Notifications" })).toBeNull();
  });

  it("is gone with the secret: after it is dismissed there is no button, no status, no text and no timer", async () => {
    const user = userEvent.setup();
    const spies = consoleSpies();
    mount();
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    await user.click(screen.getByRole("button", { name: "I have stored it" }));
    expect(screen.queryByRole("button", { name: /Copy token|Copied/ })).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(document.body.textContent).not.toMatch(/Copied|will not be shown/);
    expect(cachesHold()).not.toContain(SECRET);
    expect(toasts.getSnapshot()).toEqual([]);
    for (const spy of spies) expect(spy).not.toHaveBeenCalled();
  });

  it("clears its timer when it unmounts in the middle of the confirmation", async () => {
    userEvent.setup();
    vi.useFakeTimers();
    const spies = consoleSpies();
    const view = mount();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy token" }));
      await Promise.resolve();
    });
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    view.unmount();
    expect(vi.getTimerCount()).toBe(0);
    expect(document.body.innerHTML).not.toContain(SECRET);
    expect(cachesHold()).not.toContain(SECRET);
    expect(toasts.getSnapshot()).toEqual([]);
    for (const spy of spies) expect(spy).not.toHaveBeenCalled();
  });

  it("does not update state after it unmounted while the clipboard was still answering", async () => {
    const user = userEvent.setup();
    let release: () => void = () => undefined;
    vi.spyOn(navigator.clipboard, "writeText").mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          release = resolve;
        }),
    );
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const view = mount();
    await user.click(screen.getByRole("button", { name: "Copy token" }));
    view.unmount();
    await act(async () => {
      release();
      await Promise.resolve();
    });
    expect(errors).not.toHaveBeenCalled();
    expect(document.body.innerHTML).toBe("<div></div>");
  });
});
