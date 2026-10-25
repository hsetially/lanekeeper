import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { PrBadgeInfo } from "@/features/prs/badges";
import { prLink } from "@/features/prs/test-utils";
import { PrBadge } from "./PrBadge";

// Consequence dialogs, frame 1l: "PR #418 open" links to the pull request; "Fix merged, awaiting sync" is a dashed badge that says
// no action is needed. Each is an icon, a word and a colour. The address is the server's, so it is a link only if it is https.

const open = (over: Partial<PrBadgeInfo> = {}): PrBadgeInfo => ({ kind: "open", link: prLink(), outdated: false, ...over });
const merged = (): PrBadgeInfo => ({ kind: "merged", link: prLink({ state: "merged", pr_number: 417, closed_at: 1_760_000_000_000 }), outdated: false });

describe("PrBadge, an open pull request", () => {
  it("is a link to the pull request that opens in another tab and tells the page nothing", () => {
    render(<PrBadge info={open()} />);
    const link = screen.getByRole("link", { name: /PR #418 open/ });
    expect(link).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/418");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(link).toHaveTextContent("PR #418 open");
  });

  it("names where the link goes for a screen reader, and keeps the words that are drawn in its name", () => {
    render(<PrBadge info={open()} />);
    expect(screen.getByRole("link", { name: "PR #418 open, on GitHub" })).toBeInTheDocument();
  });

  it("says it was raised for an earlier version when the file has changed since", () => {
    render(<PrBadge info={open({ outdated: true })} />);
    const link = screen.getByRole("link", { name: /PR #418 open/ });
    expect(link).toHaveAccessibleName(/raised for an earlier version of this file/);
    expect(link).toHaveAttribute("title", expect.stringMatching(/earlier version/));
  });

  it("is not a link when the address is not https: the words are drawn, nothing is opened", () => {
    const { container } = render(<PrBadge info={open({ link: prLink({ pr_url: "javascript:alert(1)" }) })} />);
    expect(screen.queryByRole("link")).toBeNull();
    expect(container).toHaveTextContent("PR #418 open");
    expect(container.innerHTML).not.toContain("javascript:");
  });

  it("is not a link for an address with credentials", () => {
    render(<PrBadge info={open({ link: prLink({ pr_url: "https://u:p@github.com/csp/x/pull/418" }) })} />);
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("draws an icon, a word and a colour, so a state is never colour alone", () => {
    const { container } = render(<PrBadge info={open()} />);
    expect(container.querySelector("svg")).not.toBeNull();
    const badge = container.querySelector("[data-slot='pr-badge']");
    expect(badge).toHaveAttribute("data-pr-state", "open");
    expect(badge?.className).toMatch(/text-info-fg/);
  });
});

describe("PrBadge, a merged pull request", () => {
  it("says the fix merged and the swimlane has not synced, and that nothing needs doing", () => {
    const { container } = render(<PrBadge info={merged()} />);
    expect(container).toHaveTextContent("Fix merged, awaiting sync");
    const badge = container.querySelector("[data-slot='pr-badge']");
    expect(badge).toHaveAttribute("data-pr-state", "merged");
    expect(badge).toHaveAttribute("title", expect.stringMatching(/Clears on the next sync/));
    expect(badge?.className).toMatch(/border-dashed/);
    expect(container.querySelector("svg")).not.toBeNull();
  });

  it("is not a link, as the design draws it, and still names the pull request for a screen reader", () => {
    render(<PrBadge info={merged()} />);
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getByText(/PR #417 merged/)).toBeInTheDocument();
  });
});

describe("PrBadge, no pull request", () => {
  it("draws nothing", () => {
    const { container } = render(<PrBadge info={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("draws the quiet words the design uses in a table when it is asked to", () => {
    render(<PrBadge info={null} empty="No PR" />);
    expect(screen.getByText("No PR")).toBeInTheDocument();
  });
});
