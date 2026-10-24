import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { StepList } from "./StepList";

// One line for each restart request of a "restart all": the service and what the hub answered to its request, in words and with an
// icon. Never a colour alone, and never "restarted".

const svc = (name: string) => ({ namespace: "sitb", name });

describe("StepList", () => {
  const steps = [
    { service: svc("a"), state: "accepted" as const },
    { service: svc("b"), state: "sent-for-approval" as const, request: 12 },
    { service: svc("c"), state: "failed" as const },
    { service: svc("d"), state: "not-attempted" as const },
    { service: svc("e"), state: "sending" as const },
    { service: svc("f"), state: "waiting" as const },
  ];

  it("says what the hub answered to each request, in words", () => {
    render(<StepList label="Restart requests" steps={steps} />);
    const region = screen.getByRole("region", { name: "Restart requests" });
    expect(within(region).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "sitb/a accepted",
      "sitb/b sent for approval, request AP-12",
      "sitb/c not accepted",
      "sitb/d not attempted",
      "sitb/e sending",
      "sitb/f waiting",
    ]);
  });

  it("never says a service restarted", () => {
    render(<StepList label="Restart requests" steps={steps} />);
    expect(document.body.textContent).not.toMatch(/restarted|is back|ready/i);
  });

  it("marks each answer with an icon as well as words, and the pending ones with none that looks like an answer", () => {
    render(<StepList label="Restart requests" steps={steps} />);
    const items = screen.getAllByRole("listitem");
    expect(items.slice(0, 4).every((li) => li.querySelector("svg") !== null)).toBe(true);
  });

  it("scrolls with a keyboard, and shows an invisible character in a name", () => {
    render(<StepList label="Restart requests" steps={[{ service: svc("pay‮roll"), state: "accepted" }]} />);
    expect(screen.getByRole("region", { name: "Restart requests" })).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("img", { name: /U\+202E/ })).toBeInTheDocument();
  });

  it("draws nothing for no step", () => {
    const { container } = render(<StepList label="Restart requests" steps={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
});
