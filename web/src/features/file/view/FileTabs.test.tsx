import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { FileTabs } from "./FileTabs";

// The four tabs of a file: NFS (what is on the share now), Git (what the branch has), Baseline (the adopted copy) and Diff.
// A tab is a view of the same file, kept in the URL (`tab`), so it can be linked and the back button works.

describe("FileTabs", () => {
  it("has the four tabs of the design, with the selected one marked", () => {
    render(
      <FileTabs tab="git" onTab={() => undefined} gitLabel="(sit7 head)" diffCount={null}>
        <p>the body</p>
      </FileTabs>,
    );
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual(["NFS (current)", "Git (sit7 head)", "Baseline", "Diff"]);
    expect(tabs.map((t) => t.getAttribute("aria-selected"))).toEqual(["false", "true", "false", "false"]);
    expect(screen.getByText("the body")).toBeInTheDocument();
  });

  it("puts the number of changes on the Diff tab when it is known", () => {
    render(
      <FileTabs tab="nfs" onTab={() => undefined} gitLabel="(sit7 head)" diffCount={3}>
        <p>x</p>
      </FileTabs>,
    );
    expect(screen.getByRole("tab", { name: /Diff/ })).toHaveTextContent("Diff 3");
    expect(screen.getByRole("tab", { name: /Diff/ })).toHaveAccessibleName("Diff 3");
  });

  it("says which tab the person chose", async () => {
    const onTab = vi.fn();
    render(
      <FileTabs tab="nfs" onTab={onTab} gitLabel="(sit7 head)" diffCount={null}>
        <p>x</p>
      </FileTabs>,
    );
    await userEvent.click(screen.getByRole("tab", { name: /Baseline/ }));
    expect(onTab).toHaveBeenCalledWith("baseline");
  });

  it("moves between tabs with the arrow keys", async () => {
    const onTab = vi.fn();
    render(
      <FileTabs tab="nfs" onTab={onTab} gitLabel="(sit7 head)" diffCount={null}>
        <p>x</p>
      </FileTabs>,
    );
    screen.getByRole("tab", { name: /NFS/ }).focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(onTab).toHaveBeenCalledWith("git");
  });

  it("names the body by the tab, so assistive technology finds it", () => {
    render(
      <FileTabs tab="diff" onTab={() => undefined} gitLabel="(sit7 head)" diffCount={null}>
        <p>x</p>
      </FileTabs>,
    );
    expect(screen.getByRole("tabpanel")).toHaveAccessibleName("Diff");
  });
});
