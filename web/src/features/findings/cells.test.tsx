import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PathCell } from "./cells";

// The File column of the findings table: the name in bold, then its folder. Where the cell is narrower than the two, the folder gives
// way first and, if the name is still too long, it ends in an ellipsis; it is never cut off with no sign (the layout run at 1024 and 1280
// found the names cut off: `text-overflow` does not reach into a flex box).
describe("the file of a finding", () => {
  it("lets the folder shrink away and the name end in an ellipsis, so that no part of the name is cut off without a sign", () => {
    const { container } = render(<PathCell path="tx-infinity-api/tx-infinity-gateway-sit7.yml" />);
    const [name, folder] = Array.from(container.querySelectorAll<HTMLElement>("[data-slot='safe-text']"));
    expect(name?.textContent).toBe("tx-infinity-gateway-sit7.yml");
    expect(name?.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(name?.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    expect(name?.className).toMatch(/(^|\s)shrink-0(\s|$)/);
    expect(folder?.textContent).toBe("tx-infinity-api/");
    expect(folder?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
    expect(folder?.className).toMatch(/(^|\s)truncate(\s|$)/);
  });

  it("draws a file with no folder as the name alone, and a finding with no file as 'No file'", () => {
    const one = render(<PathCell path="authentication-config.yaml" />);
    expect(one.container.querySelectorAll("[data-slot='safe-text']")).toHaveLength(1);
    one.unmount();
    const none = render(<PathCell path={null} />);
    expect(none.container.textContent).toBe("No file");
  });
});
