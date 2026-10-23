import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { FileHeader } from "./FileHeader";

// The head of the file view (File, the top block): the logical file as the one h1, what makes it an effective file, its drift
// state, the line-ending flag and the facts, the path and hash with a copy button, and the actions on the right.

const base = {
  title: "tx-infinity-core.yml",
  marker: null,
  drift: null,
  note: null,
  editing: false,
  path: "tx-infinity-api/tx-infinity-core-sit7.yml",
  facts: [{ text: "CRLF" }, { text: "UTF-8" }, { text: "412 lines" }],
  hash: "a3f9c21" + "0".repeat(57),
  actions: <button type="button">An action</button>,
};

describe("FileHeader", () => {
  it("has the file's name as the page's one h1", () => {
    render(<FileHeader {...base} />);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("tx-infinity-core.yml");
  });

  it("shows the facts, the path and the hash", () => {
    render(<FileHeader {...base} />);
    for (const fact of ["CRLF", "UTF-8", "412 lines"]) expect(screen.getByText(fact)).toBeInTheDocument();
    expect(screen.getByText(/tx-infinity-core-sit7\.yml/)).toBeInTheDocument();
    expect(screen.getByText("a3f9c21")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy path" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy hash" })).toBeInTheDocument();
  });

  it("offers no copy button for the hash when the content of the file is withheld, and still shows the hash and offers the path", () => {
    render(<FileHeader {...base} copyHash={false} />);
    expect(screen.getByText("a3f9c21")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copy hash" })).toBeNull();
    expect(screen.getByRole("button", { name: "Copy path" })).toBeInTheDocument();
  });

  it("shows placeholders for the facts while they are on their way, and none when they are not coming (the file could not be read)", () => {
    const { rerender, container } = render(<FileHeader {...base} facts={null} hash={null} />);
    expect(container.querySelectorAll("[data-slot='skeleton']").length).toBeGreaterThan(0);
    rerender(<FileHeader {...base} facts={null} hash={null} unavailable />);
    expect(container.querySelectorAll("[data-slot='skeleton']")).toHaveLength(0);
  });

  it("offers a link to this file, or to this line when one is chosen, built from the route and this page's origin", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<FileHeader {...base} link="/swimlanes/sit7/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml" />);
    await user.click(screen.getByRole("button", { name: "Copy link to this file" }));
    expect(await navigator.clipboard.readText()).toBe(`${window.location.origin}/swimlanes/sit7/files?path=tx-infinity-api%2Ftx-infinity-core-sit7.yml`);
    rerender(<FileHeader {...base} link="/swimlanes/sit7/files?line=12&path=a.yml" linkLine={12} />);
    expect(screen.getByRole("button", { name: "Copy link to line 12" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copy link to this file" })).toBeNull();
  });

  it("offers no link when it has none to give", () => {
    render(<FileHeader {...base} />);
    expect(screen.queryByRole("button", { name: /Copy link/ })).toBeNull();
  });

  it("says a tenant file is the effective file for its name", () => {
    render(<FileHeader {...base} marker="Effective file for tx-infinity-core" />);
    expect(screen.getByText("Effective file for tx-infinity-core")).toBeInTheDocument();
  });

  it("shows the drift state as colour, icon and words", () => {
    render(<FileHeader {...base} drift="nfs_ahead" />);
    expect(screen.getByText("NFS ahead")).toBeInTheDocument();
  });

  it("puts the pull-request badge beside the drift state, and who changed the file at the end of the facts", () => {
    render(<FileHeader {...base} drift="nfs_ahead" prBadge={<span data-testid="pr">PR #418 open</span>} changedBy={<span data-testid="by">Changed by Priya</span>} />);
    const title = screen.getByRole("heading", { level: 1 }).parentElement as HTMLElement;
    expect(within(title).getByText("NFS ahead")).toBeInTheDocument();
    expect(within(title).getByTestId("pr")).toBeInTheDocument();
    const facts = screen.getByText("412 lines").parentElement as HTMLElement;
    expect(within(facts).getByTestId("by")).toBeInTheDocument();
    // After the hash and the link: the last thing on the facts row.
    expect(facts.lastElementChild).toContainElement(screen.getByTestId("by"));
  });

  it("draws neither slot when the screen gives none", () => {
    render(<FileHeader {...base} drift="nfs_ahead" />);
    expect(screen.queryByTestId("pr")).toBeNull();
    expect(screen.queryByTestId("by")).toBeNull();
  });

  it("flags a line-endings-only change beside the name", () => {
    render(<FileHeader {...base} note="Line endings only" />);
    expect(screen.getByText("Line endings only")).toBeInTheDocument();
  });

  it("says the file is being edited", () => {
    render(<FileHeader {...base} editing />);
    expect(screen.getByText("Editing")).toBeInTheDocument();
  });

  it("writes `Editing` in the link colour, which reads on both themes; the primary colour is 4.0:1 on the dark background (axe `color-contrast`), under 4.5:1", () => {
    render(<FileHeader {...base} editing />);
    const badge = screen.getByText("Editing").closest("[data-slot='badge']");
    expect(badge?.className).toMatch(/(^|\s)text-link(\s|$)/);
    expect(badge?.className).not.toMatch(/(^|\s)text-primary(\s|$)/);
    expect(badge?.className).toMatch(/(^|\s)border-primary(\s|$)/);
  });

  it("marks a fact the person should notice (mixed line endings)", () => {
    render(<FileHeader {...base} facts={[{ text: "Mixed endings", tone: "info" }]} />);
    expect(screen.getByText("Mixed endings")).toHaveAttribute("data-tone", "info");
  });

  it("holds the place of the facts and the hash while they load, so nothing jumps", () => {
    render(<FileHeader {...base} facts={null} hash={null} />);
    expect(screen.getByRole("heading", { level: 1 })).toBeInTheDocument();
    expect(screen.queryByText("CRLF")).not.toBeInTheDocument();
    expect(document.querySelectorAll("[data-slot='skeleton']").length).toBeGreaterThan(0);
  });

  it("puts the actions on the right", () => {
    render(<FileHeader {...base} />);
    expect(within(screen.getByRole("region", { name: "File" })).getByRole("button", { name: "An action" })).toBeInTheDocument();
  });

  it("draws a hostile file name as data", () => {
    render(<FileHeader {...base} title={"a‮b.yml"} />);
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
  });
});

describe("FileHeader, the pickup chip", () => {
  it("is at the end of the title row, after the badges the design draws, and not in the row of the facts, which has no room for it", () => {
    // The facts row holds the path, the facts, the hash, the link and who changed the file, and at 1440 wide a pickup chip in it pushed
    // "Changed by" onto a row of its own for a person whose name is "Lanekeeper user aaaaaaaa" (found by tier-1 File View 1d). In the
    // title row it comes after the drift state, the note and the Editing badge, so none of those moves (tier-1 File View 1i).
    render(<FileHeader {...base} drift="nfs_ahead" note="Line endings only" editing pickup={<span data-testid="pickup">Needs notify or restart</span>} />);
    const chip = screen.getByTestId("pickup");
    const heading = screen.getByRole("heading", { level: 1 });
    expect(chip.parentElement).toBe(heading.parentElement);
    expect(chip.parentElement).not.toBe(screen.getByText("CRLF").parentElement);
    const row = [...(chip.parentElement?.children ?? [])];
    expect(row.at(-1)).toBe(chip);
    expect(row.indexOf(chip)).toBeGreaterThan(row.indexOf(screen.getByText("Editing").closest("[data-slot='badge']") as Element));
  });

  it("is nowhere when the screen gives none", () => {
    render(<FileHeader {...base} />);
    expect(screen.queryByTestId("pickup")).toBeNull();
  });
});
