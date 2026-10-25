import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { prLink, renderPrs } from "@/features/prs/test-utils";
import { DuplicatePr, OpenPrButton } from "./DuplicatePr";

// Consequence dialogs, frame 1f: "An open PR already exists for this exact change: link only, no create button". The server
// refused a second pull request (409 `duplicate_pr`, CG13) and named the one that is open. Nothing here creates anything.


const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";

describe("DuplicatePr", () => {
  it("says the pull request is already open and that there is nothing new to create (1f)", async () => {
    renderPrs(<DuplicatePr link={prLink()} swimlane="sitb" />);
    const note = (await screen.findByText("PR #418 is already open for this change.")).closest<HTMLElement>("[data-slot='alert']");
    expect(note).not.toBeNull();
    expect(note).toHaveTextContent("Same target and same content (NFS hash aaaaaaaaaaaa), so there is nothing new to create.");
  });

  it("links to the pull request on GitHub, in another tab, and copies the same address the link opens", async () => {
    const user = userEvent.setup();
    renderPrs(<DuplicatePr link={prLink()} swimlane="sitb" />);
    const link = await screen.findByRole("link", { name: "github.com/csp/csp-tenant-data/pull/418" });
    expect(link).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/418");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    await user.click(screen.getByRole("button", { name: "Copy PR link" }));
    expect(await navigator.clipboard.readText()).toBe("https://github.com/csp/csp-tenant-data/pull/418");
  });

  it("says who raised it, and when: you, or Lanekeeper user and the start of the id (the contract names a person by id only)", async () => {
    const { unmount } = renderPrs(<DuplicatePr link={prLink({ author: ME })} swimlane="sitb" />, { user: { id: ME } });
    expect(await screen.findByText(/opened by you/)).toBeInTheDocument();
    unmount();
    renderPrs(<DuplicatePr link={prLink({ author: "99999999-9999-4999-8999-999999999999:99999999-9999-4999-8999-999999999999" })} swimlane="sitb" />, { user: { id: ME } });
    // The name is untrusted text and an element of its own (SafeText), inside the sentence.
    expect((await screen.findByText("Lanekeeper user 99999999")).parentElement).toHaveTextContent(/opened by Lanekeeper user 99999999/);
  });

  it("names the file the pull request is for, as untrusted text", async () => {
    renderPrs(<DuplicatePr link={prLink({ path: "a/b‮c.yml" })} swimlane="sitb" />);
    const marker = await screen.findByRole("img", { name: /U\+202E/i });
    expect(marker).toBeInTheDocument();
  });

  it("sends the person to the swimlane's pull requests, where every one is listed", async () => {
    renderPrs(<DuplicatePr link={prLink()} swimlane="sitb" />);
    const link = await screen.findByRole("link", { name: "All pull requests for sitb" });
    expect(link).toHaveAttribute("href", "/swimlanes/sitb/pull-requests");
  });

  it("offers no link to a pull request whose address is not https, and still says which one it is", async () => {
    renderPrs(<DuplicatePr link={prLink({ pr_url: "javascript:alert(1)" })} swimlane="sitb" />);
    expect(await screen.findByText("PR #418 is already open for this change.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /github|javascript/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Copy PR link" })).toBeNull();
    expect(screen.getByText(/The server gave no address that can be opened/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "All pull requests for sitb" })).toBeInTheDocument();
  });

  it("does not claim a pull request that is no longer open is open", async () => {
    renderPrs(<DuplicatePr link={prLink({ state: "merged", closed_at: 9 })} swimlane="sitb" />);
    expect(await screen.findByText("PR #418 is merged.")).toBeInTheDocument();
    expect(screen.queryByText(/is already open/)).toBeNull();
  });

  it("creates nothing: it has no button that raises a pull request", async () => {
    renderPrs(<DuplicatePr link={prLink()} swimlane="sitb" />);
    await screen.findByText("PR #418 is already open for this change.");
    expect(screen.queryByRole("button", { name: /create|raise/i })).toBeNull();
  });
});

describe("OpenPrButton", () => {
  it("is the footer's action: a button-looking link to the pull request", async () => {
    renderPrs(<OpenPrButton link={prLink()} />);
    const link = await screen.findByRole("link", { name: "Open PR #418" });
    expect(link).toHaveAttribute("href", "https://github.com/csp/csp-tenant-data/pull/418");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(within(link).queryByRole("img")).toBeNull();
  });

  it("is nothing for an address that cannot be opened", () => {
    const { container } = renderPrs(<OpenPrButton link={prLink({ pr_url: "http://github.com/x/pull/1" })} />);
    expect(container.querySelector("a")).toBeNull();
  });
});
