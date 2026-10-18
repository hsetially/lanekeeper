import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createRef, type ComponentProps } from "react";
import { afterEach, describe, expect, expectTypeOf, it, vi } from "vitest";
import { CopyButton } from "./CopyButton";

// The generic copy button (never for a secret: SecretCopyButton is the one for those). What a person gets: an icon with a
// "Copy" tooltip, a visible "Copied" with a check for a moment (no toast), the same thing said to a screen reader, focus left
// alone, and an honest message with the text selected when the clipboard cannot be written.

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "userActivation");
  window.getSelection()?.removeAllRanges();
});

const status = (): HTMLElement => screen.getByRole("status");

describe("CopyButton, naming", () => {
  it("is named for what it copies, and its tooltip says Copy", async () => {
    const user = userEvent.setup();
    render(<CopyButton value="req_7f3a" what="request ID" />);
    const button = screen.getByRole("button", { name: "Copy request ID" });
    await user.tab();
    expect(button).toHaveFocus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Copy");
    // No native title: it would be a second tooltip over the first.
    expect(button).not.toHaveAttribute("title");
  });

  it("is a 24 px target that takes the 18 px of the design's button in the line it sits in", () => {
    render(<CopyButton value="x" what="path" />);
    const classes = screen.getByRole("button", { name: "Copy path" }).className;
    expect(classes).toMatch(/\bsize-6\b/);
    expect(classes).toMatch(/-m-\[3px\]/);
  });

  it("must be told what it copies: an unnamed copy button cannot be written", () => {
    expectTypeOf<ComponentProps<typeof CopyButton>["what"]>().toEqualTypeOf<string>();
    expectTypeOf<ComponentProps<typeof CopyButton>["value"]>().toEqualTypeOf<string>();
  });
});

describe("CopyButton, copying", () => {
  it("puts the exact value on the clipboard, says Copied with a check, and announces it politely", async () => {
    const user = userEvent.setup();
    render(<CopyButton value="a3f9c21" what="hash" />);
    const button = screen.getByRole("button", { name: "Copy hash" });
    expect(status()).toHaveTextContent("");
    await user.click(button);
    expect(await navigator.clipboard.readText()).toBe("a3f9c21");
    expect(status()).toHaveTextContent("Copied");
    expect(status()).toHaveClass("sr-only");
    // The name does not change under the person's focus; the confirmation is the bubble and the announcement.
    expect(screen.getByRole("button", { name: "Copy hash" })).toBe(button);
    expect(button.querySelector('[data-slot="copy-check"]')).not.toBeNull();
    const bubble = document.querySelector('[data-slot="tooltip-content"]');
    expect(bubble).toHaveTextContent("Copied");
    // Not a toast.
    expect(screen.queryByRole("region", { name: "Notifications" })).toBeNull();
  });

  it("goes back after 1.5 s and the announcement empties", async () => {
    // Fully fake time: a loaded machine cannot let real time run the confirmation out while the test is still looking at it.
    userEvent.setup();
    vi.useFakeTimers();
    render(<CopyButton value="a3f9c21" what="hash" />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy hash" }));
      await Promise.resolve();
    });
    expect(status()).toHaveTextContent("Copied");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(900);
    });
    expect(status()).toHaveTextContent("Copied");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(800);
    });
    expect(status()).toHaveTextContent("");
    expect(document.querySelector('[data-slot="tooltip-content"]')).toBeNull();
    expect(screen.getByRole("button", { name: "Copy hash" }).querySelector('[data-slot="copy-check"]')).toBeNull();
  });

  it("announces a second click again, even inside the first confirmation", async () => {
    const user = userEvent.setup();
    render(<CopyButton value="v" what="hash" />);
    const button = screen.getByRole("button", { name: "Copy hash" });
    await user.click(button);
    const first = status().firstElementChild;
    await user.click(button);
    // A new node in the live region is what makes a screen reader say it again.
    expect(status().firstElementChild).not.toBe(first);
    expect(status()).toHaveTextContent("Copied");
  });

  it("never takes focus: by keyboard the button keeps it, and it stays focusable while it says Copied", async () => {
    const user = userEvent.setup();
    render(
      <>
        <button type="button">before</button>
        <CopyButton value="v" what="hash" />
      </>,
    );
    await user.tab();
    await user.tab();
    const button = screen.getByRole("button", { name: "Copy hash" });
    expect(button).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(status()).toHaveTextContent("Copied");
    expect(button).toHaveFocus();
    expect(button).not.toBeDisabled();
    expect(button).not.toHaveAttribute("aria-disabled");
    await user.keyboard(" ");
    expect(button).toHaveFocus();
  });

  it("is a labelled button when asked, and the label says Copied for a moment without changing its width", async () => {
    const user = userEvent.setup();
    render(<CopyButton value="deadbeef" what="chain hash" showLabel />);
    const button = screen.getByRole("button", { name: "Copy chain hash" });
    expect(button).toHaveTextContent("Copy chain hash");
    await user.click(button);
    expect(await navigator.clipboard.readText()).toBe("deadbeef");
    // Both words are always laid out (one is hidden from view and from assistive technology), so the button is as wide in either state.
    expect(within(button).getByText("Copied")).not.toHaveAttribute("aria-hidden");
    expect(within(button).getByText("Copy chain hash")).toHaveAttribute("aria-hidden", "true");
    // The name stays put under the person's focus.
    expect(screen.getByRole("button", { name: "Copy chain hash" })).toBe(button);
    expect(status()).toHaveTextContent("Copied");
  });

  it("closes a message with Escape", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValueOnce(new Error("denied"));
    render(<CopyButton value="x" what="path" />);
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(status()).toHaveTextContent("Couldn't copy");
    await user.keyboard("{Escape}");
    expect(status()).toHaveTextContent("");
  });
});

describe("CopyButton, when it cannot copy", () => {
  it("says so and selects the text it was given a source for, when the clipboard rejects", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValueOnce(new Error("denied"));
    const source = createRef<HTMLSpanElement>();
    render(
      <p>
        <span ref={source}>tx-infinity/core-sit1.yml</span>
        <CopyButton value="tx-infinity/core-sit1.yml" what="path" source={source} />
      </p>,
    );
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(status()).toHaveTextContent("Couldn't copy. Select the text and copy it manually.");
    expect(window.getSelection()?.toString()).toBe("tx-infinity/core-sit1.yml");
    // The bubble says it too, so a sighted person sees it.
    expect(document.querySelector('[data-slot="tooltip-content"]')).toHaveTextContent("Couldn't copy. Select the text and copy it manually.");
    // The button is still there and still works.
    expect(screen.getByRole("button", { name: "Copy path" })).toBeEnabled();
  });

  it("says so when there is no Clipboard API at all (a page that is not secure)", async () => {
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
    render(<CopyButton value="x" what="path" />);
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(status()).toHaveTextContent("Couldn't copy. Select the text and copy it manually.");
  });

  it("selects an input's text and puts the caret there, so Control+C works", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValueOnce(new Error("denied"));
    const source = createRef<HTMLInputElement>();
    render(
      <>
        <input ref={source} readOnly defaultValue="abc123" aria-label="id" />
        <CopyButton value="abc123" what="id" source={source} />
      </>,
    );
    await user.click(screen.getByRole("button", { name: "Copy id" }));
    const field = screen.getByLabelText("id");
    expect(field).toHaveFocus();
    expect((field as HTMLInputElement).selectionStart).toBe(0);
    expect((field as HTMLInputElement).selectionEnd).toBe(6);
  });

  it("does not throw into the page when the write throws synchronously", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockImplementationOnce(() => {
      throw new Error("blocked by a policy");
    });
    render(<CopyButton value="x" what="path" />);
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(status()).toHaveTextContent("Couldn't copy");
  });
});

describe("CopyButton, what it will not put on a clipboard", () => {
  it("refuses a line break or a control character in a single-line value (a pasted one runs in a terminal)", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    render(<CopyButton value={"config/app.yml\nrm -rf ~"} what="path" />);
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(write).not.toHaveBeenCalled();
    expect(status()).toHaveTextContent("Not copied: it contains a line break or control character.");
  });

  it("copies a multi-line value only when the caller says the value is text", async () => {
    const user = userEvent.setup();
    render(<CopyButton value={"line one\nline two"} what="draft" multiline />);
    await user.click(screen.getByRole("button", { name: "Copy draft" }));
    expect(await navigator.clipboard.readText()).toBe("line one\nline two");
    expect(status()).toHaveTextContent("Copied");
  });

  it("still refuses other control characters in multi-line text", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    render(<CopyButton value={"ok\u001b[2Jbad"} what="draft" multiline />);
    await user.click(screen.getByRole("button", { name: "Copy draft" }));
    expect(write).not.toHaveBeenCalled();
    expect(status()).toHaveTextContent("Not copied");
  });

  it("copies a value with a hidden character in it, and says so, because SafeText shows it as a marker", async () => {
    const user = userEvent.setup();
    render(<CopyButton value={"invoice‮gnp.exe"} what="path" />);
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(await navigator.clipboard.readText()).toBe("invoice‮gnp.exe");
    expect(status()).toHaveTextContent("Copied. It contains hidden characters.");
  });

  it("copies nothing for an empty value", async () => {
    const user = userEvent.setup();
    const write = vi.spyOn(navigator.clipboard, "writeText");
    render(<CopyButton value="" what="path" />);
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(write).not.toHaveBeenCalled();
    expect(status()).toHaveTextContent("Nothing to copy.");
  });

  it("is not for a secret: a `secret` prop renders nothing and copies nothing", () => {
    const write = vi.spyOn(navigator.clipboard, "writeText");
    // @ts-expect-error `secret` does not exist on CopyButton; a secret is copied with SecretCopyButton, from a secret display only.
    const { container } = render(<CopyButton value="lk_token_value" what="token" secret />);
    expect(container).toBeEmptyDOMElement();
    expect(write).not.toHaveBeenCalled();
  });
});

describe("CopyButton, motion and theme", () => {
  it("pops the check in with the app's own animation, which the stylesheet turns off for reduced motion", async () => {
    const user = userEvent.setup();
    render(<CopyButton value="v" what="hash" />);
    await user.click(screen.getByRole("button", { name: "Copy hash" }));
    const check = screen.getByRole("button", { name: "Copy hash" }).querySelector('[data-slot="copy-check"]');
    expect(check).toHaveClass("animate-lk-pop-in");
    const css = readFileSync(join(process.cwd(), "src/styles/base.css"), "utf8");
    const reduced = /prefers-reduced-motion: reduce[\s\S]*\.animate-lk-pop-in\s*\{\s*animation:\s*none/;
    expect(css).toMatch(reduced);
  });

  it("uses tokens for colour, so light and dark both work", async () => {
    const user = userEvent.setup();
    render(<CopyButton value="v" what="hash" />);
    const button = screen.getByRole("button", { name: "Copy hash" });
    await user.click(button);
    const html = button.outerHTML;
    expect(html).toMatch(/text-muted-foreground|text-success-fg/);
    expect(html).not.toMatch(/#[0-9a-f]{3,8}\b|rgb\(|text-(red|green|blue|gray|slate)-/i);
  });
});
