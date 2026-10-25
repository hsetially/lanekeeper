import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CopyValue } from "./CopyValue";

// "label: value [copy]": the copy button appears on hover and on focus-within, is always in the keyboard order, and copies the
// exact value (not what the value is shown as).

afterEach(() => {
  vi.restoreAllMocks();
  window.getSelection()?.removeAllRanges();
});

describe("CopyValue", () => {
  it("shows the value as SafeText and a copy button named for what it is", () => {
    render(<CopyValue value={"tx-infinity/core-sit1.yml"} what="path" />);
    const field = document.querySelector('[data-slot="copy-value"]');
    expect(field).toHaveTextContent("tx-infinity/core-sit1.yml");
    expect(within(field as HTMLElement).getByRole("button", { name: "Copy path" })).toBeInTheDocument();
  });

  it("makes hidden characters in the value visible, and still copies the value itself", async () => {
    const user = userEvent.setup();
    render(<CopyValue value={"invoice‮gnp.exe"} what="path" />);
    expect(screen.getByRole("img", { name: /U\+202E right-to-left override/ })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(await navigator.clipboard.readText()).toBe("invoice‮gnp.exe");
  });

  it("reveals the button on hover and on focus within, keeps it in the keyboard order, and shows it where there is no hover", () => {
    render(<CopyValue value="req_1" what="request ID" />);
    const button = screen.getByRole("button", { name: "Copy request ID" });
    // Hidden by opacity only: it is still a focusable button, so a keyboard user reaches it and sees it appear.
    expect(button).not.toHaveAttribute("tabindex", "-1");
    expect(button).not.toHaveAttribute("aria-hidden");
    expect(button).not.toHaveAttribute("hidden");
    const classes = button.className;
    expect(classes).toMatch(/\bopacity-0\b/);
    expect(classes).toMatch(/group-hover\/copy-value:opacity-100/);
    expect(classes).toMatch(/group-focus-within\/copy-value:opacity-100/);
    expect(classes).toMatch(/focus-visible:opacity-100/);
    expect(classes).toMatch(/\[@media\(hover:none\)\]:opacity-100/);
    // While it says Copied or could not copy, it stays in view.
    expect(classes).toMatch(/data-\[copy=copied\]:opacity-100/);
    expect(classes).toMatch(/data-\[copy=problem\]:opacity-100/);
  });

  it("is reached by Tab right after the value, and copies with Enter", async () => {
    const user = userEvent.setup();
    render(
      <>
        <button type="button">before</button>
        <CopyValue value="abc" what="hash" />
      </>,
    );
    await user.tab();
    await user.tab();
    expect(screen.getByRole("button", { name: "Copy hash" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(await navigator.clipboard.readText()).toBe("abc");
  });

  it("selects the value's own text when the clipboard says no", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValueOnce(new Error("no"));
    render(<CopyValue value="core-sit1.yml" what="path" />);
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(window.getSelection()?.toString()).toBe("core-sit1.yml");
  });

  it("copies the value when it shows something else (a short name for a long one)", async () => {
    const user = userEvent.setup();
    render(
      <CopyValue value="a3f9c21e04b7d8" what="hash">
        a3f9c21
      </CopyValue>,
    );
    expect(document.querySelector('[data-slot="copy-value"]')).toHaveTextContent("a3f9c21");
    await user.click(screen.getByRole("button", { name: "Copy hash" }));
    expect(await navigator.clipboard.readText()).toBe("a3f9c21e04b7d8");
  });
});
