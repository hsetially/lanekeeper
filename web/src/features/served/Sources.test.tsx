import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectNoAxeViolations } from "@/test/axe";
import { Sources } from "./Sources";
import { renderServed } from "./test-utils";

// The files the engine reads for the selection (`ServedFile.sources`, at most 16): what it believes the answer is made of. They are
// links to the file view, so a person can go and look at the file, and they are text from the file system, so hostile until drawn.

const SOURCES = [
  { path: "tx-infinity-api/connection-pool-sit7.yml", rank: 0 },
  { path: "tx-infinity-api/connection-pool.yml", rank: 1 },
];
const RLO = String.fromCharCode(0x202e);

describe("Sources", () => {
  it("lists each file the engine reads, with its rank, as a link to the file in the swimlane", async () => {
    renderServed(<Sources swimlane="sitb" sources={SOURCES} />);
    const list = await screen.findByRole("list", { name: "Files the engine reads for this selection" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    const first = within(items[0] as HTMLElement).getByRole("link", { name: "tx-infinity-api/connection-pool-sit7.yml" });
    expect(first).toHaveAttribute("href", `/swimlanes/sitb/files?path=${encodeURIComponent("tx-infinity-api/connection-pool-sit7.yml")}`);
    expect(items[0]).toHaveTextContent("rank 0");
    expect(items[1]).toHaveTextContent("rank 1");
  });

  it("can shrink to the room it has, so a long path ends in an ellipsis instead of running out of the page (found by the hostile layout run)", async () => {
    renderServed(<Sources swimlane="sitb" sources={SOURCES} />);
    const list = await screen.findByRole("list", { name: "Files the engine reads for this selection" });
    // A flex item does not shrink below its content unless it says min-w-0, and the list is one: without it a path of 1,700 px keeps the
    // list that wide and the footer cuts the link off at the card's edge with no ellipsis.
    expect(list.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    for (const item of within(list).getAllByRole("listitem")) {
      expect(item.className).toMatch(/(^|\s)max-w-full(\s|$)/);
      expect(within(item).getByRole("link").className).toMatch(/(^|\s)truncate(\s|$)/);
    }
  });

  it("makes each link a target of 24 px and bounds the list, so that sixteen long paths neither fail the target size nor push the text out of the page (found by the accessibility run: 16 sources at 1440 wide, and a text 0 px high at 1024 wide)", async () => {
    renderServed(<Sources swimlane="sitb" sources={SOURCES} />);
    const list = await screen.findByRole("list", { name: "Files the engine reads for this selection" });
    // WCAG 2.5.8 (axe `target-size`): a link of 16 px with 4 px between it and the next is too small and too close; a line of 24 px is a
    // target of 24 px whatever is around it.
    for (const item of within(list).getAllByRole("listitem")) expect(within(item).getByRole("link").className).toMatch(/(^|\s)leading-6(\s|$)/);
    // The hub sends up to 16 and a path can be long: the list holds a few lines and scrolls inside its own box.
    expect(list.className).toMatch(/(^|\s)max-h-28(\s|$)/);
    expect(list.className).toMatch(/(^|\s)overflow-y-auto(\s|$)/);
  });

  it("draws nothing when the engine has no source for the selection", async () => {
    renderServed(
      <>
        <Sources swimlane="sitb" sources={[]} />
        <p>after</p>
      </>,
    );
    await screen.findByText("after");
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("draws a hostile path as text with its hidden characters made visible, and a link only to the app", async () => {
    renderServed(<Sources swimlane="sitb" sources={[{ path: `a/javascript:alert(1)${RLO}.yml`, rank: 0 }]} />);
    const link = await screen.findByRole("link");
    expect(link.getAttribute("href")).toMatch(/^\/swimlanes\/sitb\/files\?path=/);
    expect(link.getAttribute("href")).not.toMatch(/^javascript:/);
    expect(within(link).getAllByRole("img", { name: /U\+202E/i })).toHaveLength(1);
    expect(document.body.textContent).not.toContain(RLO);
  });

  it("makes no link for a swimlane the route table does not accept, and still shows the path", async () => {
    renderServed(<Sources swimlane="Not A Swimlane" sources={SOURCES} />);
    expect(await screen.findByText("tx-infinity-api/connection-pool-sit7.yml")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("has no structural accessibility violation", async () => {
    renderServed(<Sources swimlane="sitb" sources={SOURCES} />);
    await screen.findByRole("list");
    await expectNoAxeViolations();
  });
});
