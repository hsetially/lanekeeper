import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { STATUS } from "@/handoff/pairs.generated";
import { STATUS_VARIANTS, StatusBadge, type StatusVariant } from "./StatusBadge";

const HANDOFF_DRIFT_AND_OPS = STATUS.filter((s) => s.key !== "restart");

describe("StatusBadge vocabulary", () => {
  it("covers the nine handoff statuses with the handoff's label and icon, in order", () => {
    for (const s of HANDOFF_DRIFT_AND_OPS) {
      const v = STATUS_VARIANTS[s.key as StatusVariant];
      expect(v, s.key).toBeDefined();
      expect(v.label, s.key).toBe(s.label);
      expect(v.icon, s.key).toBe(s.icon);
      expect(v.dashed, s.key).toBe(s.border === "dashed");
    }
  });

  it("replaces the handoff's 'Pending restart' with the four pickup states (D85)", () => {
    expect("restart" in STATUS_VARIANTS).toBe(false);
    expect(Object.keys(STATUS_VARIANTS)).toEqual(
      expect.arrayContaining(["live", "live-by", "needs-notify-or-restart", "needs-config-server-restart"]),
    );
    expect(Object.keys(STATUS_VARIANTS)).toHaveLength(13);
    expect(STATUS_VARIANTS["needs-notify-or-restart"].label).toBe("Needs notify or restart");
    expect(STATUS_VARIANTS["needs-config-server-restart"].label).toBe("Needs config-server restart");
    expect(STATUS_VARIANTS.live.label).toBe("Live");
  });
});

describe("StatusBadge rendering: colour plus icon plus label, never colour alone", () => {
  for (const variant of Object.keys(STATUS_VARIANTS) as StatusVariant[]) {
    it(`${variant} shows an icon, its label and its own token colours`, () => {
      const { container, unmount } = render(<StatusBadge variant={variant} />);
      const badge = container.firstElementChild as HTMLElement;
      expect(badge.querySelector("svg")).not.toBeNull();
      expect(badge).toHaveTextContent(STATUS_VARIANTS[variant].label);
      expect(badge).toHaveAttribute("data-status", variant);
      const tokens = STATUS_VARIANTS[variant].tokens;
      expect(badge.className).toContain(`text-status-${tokens}-fg`);
      expect(badge.className).toContain(`bg-status-${tokens}-bg`);
      expect(badge.className).toContain(`border-status-${tokens}-border`);
      unmount();
    });
  }

  it("ends its label in an ellipsis when the room is short, and never grows past its container (the layout run, labels 40% longer: 'Needs notify or restart' ran under the Restart button)", () => {
    render(<StatusBadge variant="needs-notify-or-restart" />);
    const badge = screen.getByText("Needs notify or restart").closest("[data-slot='status-badge']");
    expect(badge?.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    const label = screen.getByText("Needs notify or restart");
    expect(label.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(label.className).toMatch(/(^|\s)truncate(\s|$)/);
  });

  it("draws Untracked with a dashed border and nothing else", () => {
    const { container } = render(
      <div>
        <StatusBadge variant="untracked" />
        <StatusBadge variant="sync" />
      </div>,
    );
    const [untracked, sync] = Array.from(container.querySelectorAll("[data-status]"));
    expect(untracked?.className).toContain("border-dashed");
    expect(sync?.className).not.toContain("border-dashed");
  });

  it("shows a count, and in compact form keeps the label for assistive technology", () => {
    const { rerender } = render(<StatusBadge variant="git" count={4} />);
    expect(screen.getByText("4")).toBeInTheDocument();
    expect(screen.getByText("Git ahead")).toBeInTheDocument();
    rerender(<StatusBadge variant="git" count={4} compact />);
    const badge = screen.getByLabelText("4 Git ahead");
    expect(badge).toHaveAttribute("title", "Git ahead");
    expect(badge.textContent).toBe("4");
  });

  it("lets the live variant say by when (Live by 14:32)", () => {
    render(<StatusBadge variant="live-by" detail="14:32" />);
    expect(screen.getByText("Live by 14:32")).toBeInTheDocument();
  });

  it("can take another label for the same variant (PR open reuses git, Adopted reuses sync)", () => {
    render(<StatusBadge variant="git" label="PR open" />);
    expect(screen.getByText("PR open")).toBeInTheDocument();
  });
});
