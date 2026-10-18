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
