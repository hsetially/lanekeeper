import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Tabs, TabsList, TabsTrigger } from "./tabs";

// The underline tabs. Where the room is short (a file view beside a side panel at 1024 wide, labels 40% longer) the tabs go to a second
// line; they do not run under the panel next to them.
describe("the tab list", () => {
  it("wraps its tabs onto another line when they do not fit, and keeps the bar at least as high as one tab", () => {
    render(
      <Tabs defaultValue="a">
        <TabsList aria-label="Versions">
          <TabsTrigger value="a">One</TabsTrigger>
          <TabsTrigger value="b">Two</TabsTrigger>
        </TabsList>
      </Tabs>,
    );
    const list = screen.getByRole("tablist", { name: "Versions" });
    expect(list.className).toMatch(/(^|\s)flex-wrap(\s|$)/);
  });
});
