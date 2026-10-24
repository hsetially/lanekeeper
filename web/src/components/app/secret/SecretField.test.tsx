import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { SecretField } from "./SecretField";

// The read-only field that shows a one-time secret: a person who cannot use the copy button can still take the whole token with
// the keyboard or the pointer. Focus, a click and a double click all select all of it (a token has hyphens and dots, which end a
// "word" for the browser's own double click).

const TOKEN = "lkpat_ZxY9w8V7u6T5s4R3q2P1o0N-mLkJiHgFe.DcBa";

const selected = (field: HTMLInputElement): string => field.value.slice(field.selectionStart ?? 0, field.selectionEnd ?? 0);

describe("SecretField", () => {
  it("is read-only, not completed or spell-checked, and set in mono", () => {
    render(<SecretField aria-label="Your token" value={TOKEN} />);
    const field = screen.getByLabelText("Your token");
    expect(field).toHaveAttribute("readonly");
    expect(field).toHaveAttribute("autocomplete", "off");
    expect(field).toHaveAttribute("spellcheck", "false");
    expect(field).toHaveClass("font-mono");
    expect(field).toHaveValue(TOKEN);
  });

  it("selects the whole token when it takes focus", async () => {
    const user = userEvent.setup();
    render(<SecretField aria-label="Your token" value={TOKEN} />);
    await user.tab();
    const field = screen.getByLabelText<HTMLInputElement>("Your token");
    expect(field).toHaveFocus();
    expect(selected(field)).toBe(TOKEN);
  });

  it("selects the whole token on a double click, hyphens and dots included", async () => {
    const user = userEvent.setup();
    render(<SecretField aria-label="Your token" value={TOKEN} />);
    const field = screen.getByLabelText<HTMLInputElement>("Your token");
    await user.dblClick(field);
    expect(selected(field)).toBe(TOKEN);
  });

  it("selects the whole token on a single click too, so the next Control+C takes all of it", async () => {
    const user = userEvent.setup();
    render(<SecretField aria-label="Your token" value={TOKEN} />);
    const field = screen.getByLabelText<HTMLInputElement>("Your token");
    await user.click(field);
    expect(selected(field)).toBe(TOKEN);
  });

  it("passes a ref and the attributes a Field gives it", () => {
    let seen: HTMLInputElement | null = null;
    render(<SecretField ref={(element) => { seen = element; }} id="t" aria-describedby="d" aria-label="x" value={TOKEN} />);
    expect(seen).toBeInstanceOf(HTMLInputElement);
    expect(screen.getByLabelText("x")).toHaveAttribute("aria-describedby", "d");
  });
});
