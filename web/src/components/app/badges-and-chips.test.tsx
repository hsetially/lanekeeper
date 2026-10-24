import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { AgentDot } from "./AgentDot";
import { BranchChip, HashChip, PathChip, SettingChip } from "./chips";
import { TierBadge } from "./TierBadge";
import { truncateMiddle } from "./truncate";

describe("truncateMiddle (the Style Guide's trunc)", () => {
  it("leaves short text alone", () => {
    expect(truncateMiddle("sitb", 10)).toBe("sitb");
    expect(truncateMiddle("0123456789", 10)).toBe("0123456789");
  });

  it("keeps about 40% from the start and 60% from the end, and exactly max characters", () => {
    const out = truncateMiddle("tx-infinity-api/tx-infinity-core-sit1.yml", 20);
    // 8 characters from the start, the ellipsis, 11 from the end: exactly 20.
    expect(out).toBe("tx-infin…re-sit1.yml");
    expect(Array.from(out)).toHaveLength(20);
  });

  it("never splits a surrogate pair and never exceeds the limit", () => {
    const text = "a".repeat(5) + "\u{1F600}".repeat(20) + "b".repeat(5);
    const out = truncateMiddle(text, 12);
    expect(Array.from(out).length).toBeLessThanOrEqual(12);
    expect(out).not.toMatch(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/);
    expect(out.startsWith("aaaa")).toBe(true);
    expect(out.endsWith("bbbbb")).toBe(true);
  });

  it("holds for any length", () => {
    for (let len = 0; len < 80; len += 7) {
      for (const max of [3, 8, 20]) {
        const out = truncateMiddle("x".repeat(len), max);
        expect(Array.from(out).length).toBeLessThanOrEqual(max);
      }
    }
  });
});

describe("chips", () => {
  it("PathChip shows a truncated path, keeps the full path for assistive technology and copies it whole", async () => {
    const user = userEvent.setup();
    const path = "tx-infinity-api/deeply/nested/folder/tx-infinity-core-sit1.yml";
    render(<PathChip path={path} maxChars={24} />);
    const visible = screen.getByText(/…/);
    expect(visible.closest('[aria-hidden="true"]')).not.toBeNull();
    expect(Array.from(visible.textContent ?? "").length).toBeLessThanOrEqual(24);
    expect(screen.getByText(path, { selector: ".sr-only, .sr-only *" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(await navigator.clipboard.readText()).toBe(path);
  });

  it("PathChip shows a short path whole", () => {
    render(<PathChip path="app.yml" />);
    expect(screen.getByText("app.yml")).toBeInTheDocument();
    expect(screen.queryByText(/…/)).not.toBeInTheDocument();
  });

  it("PathChip makes hidden characters visible (S8)", () => {
    render(<PathChip path={"invoice\u202Egnp.exe"} copy={false} />);
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
  });

  it("HashChip shows seven characters and copies all of them", async () => {
    const user = userEvent.setup();
    const hash = "a3f9c21d8e7b6a5948372615049382716abcdef0123456789abcdef012345678";
    render(<HashChip hash={hash} />);
    expect(screen.getByText("a3f9c21")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy hash" }));
    expect(await navigator.clipboard.readText()).toBe(hash);
  });

  it("BranchChip and SettingChip render their value in mono", () => {
    render(
      <div>
        <BranchChip branch="sit7" />
        <SettingChip path="txInfinityOptions.remote-itm-teller.enableAccountSorting" />
      </div>,
    );
    expect(screen.getByText("sit7")).toHaveClass("font-mono");
    expect(screen.getByRole("button", { name: "Copy branch" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy setting path" })).toBeInTheDocument();
  });
});

describe("TierBadge, AgentDot", () => {
  it("TierBadge outlines SIT and fills PRESIT", () => {
    const { rerender } = render(<TierBadge tier="sit" />);
    expect(screen.getByText("SIT")).toHaveAttribute("data-variant", "outline");
    rerender(<TierBadge tier="presit" />);
    expect(screen.getByText("PRESIT")).toHaveAttribute("data-variant", "muted");
  });

  it("AgentDot differs by shape as well as colour: filled, ring, diamond", () => {
    const shapes = (["online", "degraded", "offline"] as const).map((state) => {
      const { container, unmount } = render(<AgentDot state={state} />);
      const dot = container.firstElementChild as HTMLElement;
      const shape = dot.getAttribute("data-shape");
      expect(dot).toHaveAttribute("role", "img");
      expect(dot.getAttribute("aria-label")).toMatch(new RegExp(state, "i"));
      unmount();
      return shape;
    });
    expect(shapes).toEqual(["filled", "ring", "diamond"]);
  });
});
