import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { HiddenValue } from "./HiddenValue";

// A value the server hid is drawn as a TOKEN: an icon and the words "Hidden value", never colour alone (the design's status rule).
// A screen reader hears "Hidden value", and the setting's name when the mask names one. The token holds nothing of the value: the
// text is fixed, and the marker's own words (its length) are all that comes from the server.

describe("HiddenValue", () => {
  it("shows an icon and the words, and says how long the hidden value was", () => {
    const { container } = render(<HiddenValue info={{ chars: 22, rule: "generic_secret" }} />);
    const token = container.querySelector("[data-slot='hidden-value']");
    expect(token).not.toBeNull();
    expect(token?.querySelector("svg[aria-hidden='true']")).not.toBeNull();
    expect(token).toHaveTextContent("Hidden value");
    expect(token).toHaveTextContent("22 chars");
  });

  it("names the setting for a screen reader when there is one, and says nothing of a setting when there is none", () => {
    const { rerender, container } = render(<HiddenValue info={{ chars: 3, rule: "x" }} setting="txInfinityOptions.remote-itm-teller.clientSecret" />);
    expect(container.textContent).toContain("Hidden value");
    expect(container.querySelector(".sr-only")?.textContent).toBe(" for setting txInfinityOptions.remote-itm-teller.clientSecret");
    rerender(<HiddenValue info={{ chars: 3, rule: "x" }} />);
    expect(container.querySelector(".sr-only")).toBeNull();
  });

  it("makes hidden characters of a setting name visible", () => {
    render(<HiddenValue info={null} setting={"a\u202eb"} />);
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
  });

  it("copes with a marker whose length is unknown", () => {
    const { container } = render(<HiddenValue info={{ chars: null, rule: "x" }} />);
    expect(container.textContent).toBe("Hidden value");
  });

  it("has no title, no data attribute and no label that carries anything but its fixed words", () => {
    const { container } = render(<HiddenValue info={{ chars: 22, rule: "generic_secret" }} setting="a.b" />);
    const attributes = [...container.querySelectorAll("*")].flatMap((el) => [...el.attributes].map((a) => `${a.name}=${a.value}`));
    expect(attributes.filter((a) => /^(title|aria-label|data-(?!slot))/.test(a))).toEqual([]);
  });

  it("has no axe violations", async () => {
    const { container } = render(
      <p>
        <HiddenValue info={{ chars: 5, rule: "x" }} setting="a.b" />
      </p>,
    );
    await expectNoAxeViolations(container);
  });
});
