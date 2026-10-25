import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { productSources } from "@/test/source-files";
import { OfflineLanesBanner } from "./OfflineLanesBanner";

// The notice above a comparison when the agent behind a column is offline. The two compare screens (the lanes view and the grid)
// say different things about the age of the files, and used to carry a copy each of everything else.

describe("OfflineLanesBanner", () => {
  it("names the swimlane when one agent is offline, and offers a Retry that names it", async () => {
    const onRetry = vi.fn();
    render(
      <OfflineLanesBanner lanes={[{ id: "sitb" }]} onRetry={onRetry}>
        Its column shows the NFS files from the last scan.
      </OfflineLanesBanner>,
    );
    expect(screen.getByText("sitb agent is offline")).toBeInTheDocument();
    expect(screen.getByText("Its column shows the NFS files from the last scan.")).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry sitb" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("counts the agents when there are several, and Retry is the plain word", () => {
    render(
      <OfflineLanesBanner lanes={[{ id: "sit" }, { id: "sitb" }, { id: "sitc" }]} onRetry={() => undefined}>
        Their columns show the NFS files of their last scan.
      </OfflineLanesBanner>,
    );
    expect(screen.getByText("3 agents are offline")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("draws nothing when no agent is offline", () => {
    const { container } = render(
      <OfflineLanesBanner lanes={[]} onRetry={() => undefined}>
        x
      </OfflineLanesBanner>,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("is the one banner of its kind in the compare screens", () => {
    const own = productSources().filter(({ path, text }) => path.startsWith("src/features/compare/") && /icon=\{<WifiOff/.test(text)).map((f) => f.path);
    expect(own).toEqual(["src/features/compare/OfflineLanesBanner.tsx"]);
  });
});
