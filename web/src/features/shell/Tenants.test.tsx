import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { TenantSet } from "./Tenants";

// The tenants of a swimlane, wherever one is drawn (the list, the switcher, the header, the search hits). A tenant name is the hub's
// text and has no length limit that the screen can rely on: the layout run put a 1,700px one in and the cell cut it off with no sign.
describe("the tenant set", () => {
  const draw = (tenants: string[]) =>
    render(
      <TooltipProvider>
        <TenantSet tenants={tenants} />
      </TooltipProvider>,
    );

  it("lets the first tenant end in an ellipsis in the room it is given, and keeps the +N chip", () => {
    const { container } = draw(["a-tenant-name-that-goes-on-and-on-and-on", "sit8", "sit9"]);
    const root = container.querySelector("[data-slot='tenant-set']");
    expect(root?.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    expect(root?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    const first = root?.querySelector("[data-slot='safe-text']");
    expect(first?.textContent).toBe("a-tenant-name-that-goes-on-and-on-and-on");
    expect(first?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(first?.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(root?.textContent).toContain("+2");
  });

  it("says there are none for a swimlane without a tenant", () => {
    const { container } = draw([]);
    expect(container.querySelector("[data-slot='tenant-set']")?.textContent).toBe("No tenants");
  });
});
