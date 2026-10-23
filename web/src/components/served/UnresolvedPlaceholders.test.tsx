import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { UnresolvedPlaceholders } from "./UnresolvedPlaceholders";

// The placeholders the config-server left in the text (`${KEY}` where its property view had no such key): by definition the ones it
// did not replace. The engine's list says which; the text itself shows where.

describe("UnresolvedPlaceholders", () => {
  it("says nothing when every placeholder was replaced", () => {
    const { container } = render(<UnresolvedPlaceholders placeholders={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("names the placeholders that stayed, as they are written in the text, with a title that counts them", () => {
    render(<UnresolvedPlaceholders placeholders={["AUDIT_SINK", "TENANT_REGION"]} />);
    expect(screen.getByText("2 placeholders left as written")).toBeInTheDocument();
    expect(screen.getByText("${AUDIT_SINK}")).toBeInTheDocument();
    expect(screen.getByText("${TENANT_REGION}")).toBeInTheDocument();
  });

  it("says one placeholder in the singular", () => {
    render(<UnresolvedPlaceholders placeholders={["AUDIT_SINK"]} />);
    expect(screen.getByText("1 placeholder left as written")).toBeInTheDocument();
  });

  it("is a status with an icon and words, never colour alone", () => {
    render(<UnresolvedPlaceholders placeholders={["A"]} />);
    const status = screen.getByRole("status");
    expect(status.querySelector("svg")).not.toBeNull();
  });

  it("names the first ones and counts the rest, so ten thousand placeholders are not ten thousand elements", () => {
    const many = Array.from({ length: 10_000 }, (_, i) => `K${i}`);
    render(<UnresolvedPlaceholders placeholders={many} />);
    expect(screen.getByText("10,000 placeholders left as written")).toBeInTheDocument();
    expect(screen.getByText("${K0}")).toBeInTheDocument();
    expect(screen.getByText(/and 9,988 more/)).toBeInTheDocument();
    expect(document.querySelectorAll("[data-slot='placeholder']").length).toBeLessThanOrEqual(12);
  });

  it("names the keys in a box of its own that holds a few lines and scrolls, which a keyboard can reach (found by the accessibility run: forty long keys took 298 px and left the text 0 px high at 1024 wide)", () => {
    render(<UnresolvedPlaceholders placeholders={Array.from({ length: 12 }, (_, i) => `INTEGRATION_${i}_ENDPOINT_BASE_URL_FOR_THE_PARTNER_GATEWAY`)} />);
    const box = screen.getByRole("region", { name: "Placeholders left as written" });
    expect(box.className).toMatch(/(^|\s)max-h-24(\s|$)/);
    expect(box.className).toMatch(/(^|\s)overflow-y-auto(\s|$)/);
    expect(box).toHaveAttribute("tabindex", "0");
    expect(box.querySelectorAll("[data-slot='placeholder']")).toHaveLength(12);
  });

  it("counts a key once however often the engine listed it", () => {
    render(<UnresolvedPlaceholders placeholders={["A", "A", "B"]} />);
    expect(screen.getByText("2 placeholders left as written")).toBeInTheDocument();
  });

  it("draws a hostile key as text with its hidden characters made visible, never as markup", () => {
    render(<UnresolvedPlaceholders placeholders={["<img src=x onerror=alert(1)>", "a‮b"]} />);
    expect(document.querySelector("img")).toBeNull();
    expect(screen.getByText("${<img src=x onerror=alert(1)>}")).toBeInTheDocument();
    expect(screen.getAllByRole("img", { name: /U\+202E/i })).toHaveLength(1);
  });

  it("has no structural accessibility violation", async () => {
    render(<UnresolvedPlaceholders placeholders={["AUDIT_SINK"]} />);
    await expectNoAxeViolations();
  });
});
