import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderFile } from "../test-utils";
import { AgentOfflineBanner, ChangedWhileEditingBanner, EditingStrip, LargeFileBanner, ReadOnlyBanner, WithheldBanner } from "./Banners";

const NOW = Date.UTC(2026, 9, 17, 10, 42);

describe("ReadOnlyBanner", () => {
  it("says what a Viewer can do and what needs Editor (States P03)", async () => {
    renderFile(<ReadOnlyBanner />);
    expect(await screen.findByText("Read-only")).toBeInTheDocument();
    expect(screen.getByText("Viewers can compare and download. Saving and raising PRs need Editor.")).toBeInTheDocument();
  });
});

describe("AgentOfflineBanner", () => {
  it("says when the data is from and that saving is off (States O03)", async () => {
    renderFile(<AgentOfflineBanner lane="sitb" asOf={Date.UTC(2026, 9, 17, 10, 28)} now={NOW} canCheck />);
    expect(await screen.findByText("Agent for sitb is offline")).toBeInTheDocument();
    expect(screen.getByText(/Showing data as of 10:28 UTC \(14 min ago\)\. Saving on sitb is off\./)).toBeInTheDocument();
  });
  it("sends an Admin to the agents", async () => {
    renderFile(<AgentOfflineBanner lane="sitb" asOf={null} now={NOW} canCheck />);
    expect(await screen.findByRole("link", { name: "Check agent" })).toHaveAttribute("href", "/admin/swimlanes");
  });
  it("offers nothing to check to someone who cannot open the agents", async () => {
    renderFile(<AgentOfflineBanner lane="sitb" asOf={null} now={NOW} canCheck={false} />);
    await screen.findByText("Agent for sitb is offline");
    expect(screen.queryByRole("link", { name: "Check agent" })).not.toBeInTheDocument();
    expect(screen.getByText(/Showing the data from the last scan/)).toBeInTheDocument();
  });
});

describe("WithheldBanner", () => {
  it("says the content is withheld and what is shown instead", async () => {
    renderFile(<WithheldBanner />);
    expect(await screen.findByText("Content withheld: sensitive file type")).toBeInTheDocument();
    expect(screen.getByText(/shows only the name, size, hash and who changed it/)).toBeInTheDocument();
  });
});

describe("LargeFileBanner", () => {
  it("says the file opened read-only first, and enables editing on request", async () => {
    const onEnable = vi.fn();
    renderFile(<LargeFileBanner lines={6140} gate={{ allowed: true, reason: null }} onEnableEditing={onEnable} />);
    expect(await screen.findByText("Large file · 6,140 lines")).toBeInTheDocument();
    expect(screen.getByText("Opened read-only first so the page stays fast.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Enable editing" }));
    expect(onEnable).toHaveBeenCalledTimes(1);
  });
  it("disables that button with its reason for a Viewer", async () => {
    renderFile(<LargeFileBanner lines={6140} gate={{ allowed: false, reason: "Needs the Editor role. Ask an Admin to grant it." }} onEnableEditing={() => undefined} />);
    expect(await screen.findByRole("button", { name: "Enable editing" })).toHaveAttribute("aria-disabled", "true");
  });
});

describe("EditingStrip", () => {
  it("says the save goes straight to NFS and Git changes only by a PR", async () => {
    renderFile(<EditingStrip lane="sitb" file="tx-infinity-core-sit7.yml" />);
    expect(await screen.findByText("Editing NFS")).toBeInTheDocument();
    expect(screen.getByText("Saving writes straight to the sitb NFS share. Git changes only through a PR.")).toBeInTheDocument();
  });
});

describe("ChangedWhileEditingBanner", () => {
  it("warns before the save does that someone else changed the file", async () => {
    renderFile(<ChangedWhileEditingBanner />);
    expect(await screen.findByText("This file changed on NFS while you were editing")).toBeInTheDocument();
    expect(screen.getByText(/Saving will be refused/)).toBeInTheDocument();
  });
});
