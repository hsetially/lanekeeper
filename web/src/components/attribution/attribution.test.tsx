import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { describe, expect, it } from "vitest";
import { Confidence, type Attribution, type Me } from "@/api/generated/model";
import { SessionContext } from "@/auth/session";
import { me } from "@/test/app";
import { expectNoAxeViolations } from "@/test/axe";
import { AttributionChip } from "./AttributionChip";
import { AttributionEvidence } from "./AttributionEvidence";
import { AttributionSourceCell } from "./AttributionSourceCell";
import { ConfidenceMeter } from "./ConfidenceMeter";
import { UnknownAuthorBadge } from "./UnknownAuthorBadge";

// The attribution chip and its parts: who changed a file, how Lanekeeper knows, how sure it is, and the evidence behind it. Each
// state is a real attribution of the contract; none is made up by the component.

const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const OTHER = "11111111-1111-4111-8111-111111111111:3f2a91bc-0000-4000-8000-000000000003";

const attribution = (over: Partial<Attribution> = {}): Attribution => ({ source: "tool_write", confidence: "certain", actor: { kind: "user", user: ME }, evidence: {}, ...over });

function signedIn(ui: ReactElement, id: string = ME) {
  const session = { status: "ready" as const, me: { ...me("editor"), id } as unknown as Me, role: "editor" as const };
  return render(<SessionContext.Provider value={session}>{ui}</SessionContext.Provider>);
}

const filled = (root: HTMLElement) => root.querySelectorAll("[data-filled='true']").length;

describe("ConfidenceMeter, four bars", () => {
  it.each([
    ["low", 1, "Low"],
    ["medium", 2, "Medium"],
    ["high", 3, "High"],
    ["certain", 4, "Certain"],
  ] as const)("fills %s with %i of four bars and says %s in words", (level, step, word) => {
    const { container } = render(<ConfidenceMeter confidence={level} />);
    const meter = container.firstElementChild as HTMLElement;
    expect(meter).toHaveAttribute("data-slot", "confidence");
    expect(meter).toHaveAttribute("data-level", level);
    expect(meter).toHaveAttribute("data-step", String(step));
    expect(container.querySelectorAll("[data-bar]")).toHaveLength(4);
    expect(filled(meter)).toBe(step);
    expect(meter).toHaveTextContent(`Confidence: ${word}`);
    expect(meter).toHaveAttribute("title", `Attribution confidence: ${word}`);
  });

  it("draws a filled bar at full strength and an empty one faint: what is painted is what the step says", () => {
    for (const [level, step] of [["low", 1], ["medium", 2], ["high", 3], ["certain", 4], [null, 0]] as const) {
      const { container, unmount } = render(<ConfidenceMeter confidence={level} />);
      const bars = Array.from(container.querySelectorAll("[data-bar]"));
      expect(bars.map((bar) => bar.className.includes("opacity-100"))).toEqual([0, 1, 2, 3].map((i) => i < step));
      expect(bars.map((bar) => bar.className.includes("opacity-25"))).toEqual([0, 1, 2, 3].map((i) => i >= step));
      unmount();
    }
  });

  it("hides the bars from assistive technology: the word is the meaning, the bars are the quick read", () => {
    const { container } = render(<ConfidenceMeter confidence="high" />);
    expect(container.querySelector("[aria-hidden='true']")).not.toBeNull();
    expect(container.querySelectorAll("[data-bar]")).toHaveLength(4);
    for (const bar of container.querySelectorAll("[data-bar]")) expect(bar.closest("[aria-hidden='true']")).not.toBeNull();
  });

  it("draws no score as a dashed outline with empty bars and the word Unknown, so it differs from every level without colour", () => {
    const { container } = render(<ConfidenceMeter confidence={null} />);
    const meter = container.firstElementChild as HTMLElement;
    expect(meter).toHaveAttribute("data-level", "none");
    expect(meter).toHaveAttribute("data-step", "0");
    expect(meter.className).toContain("border-dashed");
    expect(filled(meter)).toBe(0);
    expect(meter).toHaveTextContent("Unknown");
    const solid = render(<ConfidenceMeter confidence="low" />).container.firstElementChild as HTMLElement;
    expect(solid.className).not.toContain("border-dashed");
  });
});

describe("ConfidenceMeter, quarter-step track", () => {
  it("is a track of four quarters, filled by the same steps", () => {
    for (const [level, step] of [["low", 1], ["medium", 2], ["high", 3], ["certain", 4]] as const) {
      const { container, unmount } = render(<ConfidenceMeter variant="track" confidence={level} />);
      const meter = container.firstElementChild as HTMLElement;
      expect(meter).toHaveAttribute("data-variant", "track");
      expect(container.querySelectorAll("[data-bar]")).toHaveLength(4);
      expect(filled(meter)).toBe(step);
      unmount();
    }
  });

  it("paints exactly the steps it says, and leaves the rest as the plain track", () => {
    for (const [level, step] of [["low", 1], ["medium", 2], ["high", 3], ["certain", 4]] as const) {
      const { container, unmount } = render(<ConfidenceMeter variant="track" confidence={level} />);
      const bars = Array.from(container.querySelectorAll("[data-bar]"));
      expect(bars.map((bar) => /\bbg-(foreground|warning-fg)\b/.test(bar.className))).toEqual([0, 1, 2, 3].map((i) => i < step));
      // The track under them is the plain accent colour, so a step that is not filled shows it.
      expect(bars[0]?.parentElement?.className).toContain("bg-accent");
      unmount();
    }
  });

  it("tints the weaker half (low, medium) with the warning colour and the stronger half with the foreground", () => {
    const fill = (level: Confidence) => render(<ConfidenceMeter variant="track" confidence={level} />).container.querySelector("[data-filled='true']")?.className ?? "";
    expect(fill("low")).toContain("bg-warning-fg");
    expect(fill("medium")).toContain("bg-warning-fg");
    expect(fill("high")).toContain("bg-foreground");
    expect(fill("certain")).toContain("bg-foreground");
  });

  it("uses no inline style: the steps are classes, which the CSP allows", () => {
    const { container } = render(<ConfidenceMeter variant="track" confidence="medium" />);
    expect(container.querySelector("[style]")).toBeNull();
  });
});

describe("AttributionChip", () => {
  it("says You, via Lanekeeper and Certain for the signed-in person's own tool write", () => {
    signedIn(<AttributionChip attribution={attribution()} />);
    const chip = screen.getByRole("button");
    expect(chip).toHaveTextContent("You");
    expect(chip).toHaveTextContent("via Lanekeeper");
    expect(chip).toHaveTextContent("Confidence: Certain");
    expect(chip.closest("[data-slot='attribution']")).toHaveAttribute("data-who", "you");
  });

  it("sets the design's middle dot before the continuing phrase, hidden from assistive technology, and none before a bracketed note", () => {
    const { container, unmount } = signedIn(<AttributionChip attribution={attribution()} />);
    const dot = Array.from(container.querySelectorAll("[aria-hidden='true']")).find((el) => (el.textContent ?? "").trim() === "·");
    expect(dot).toBeDefined();
    expect(dot?.parentElement).toHaveTextContent("via Lanekeeper");
    unmount();
    const client = render(<AttributionChip attribution={attribution({ source: "nfs_client", confidence: "medium", actor: null })} />).container;
    expect(Array.from(client.querySelectorAll("[aria-hidden='true']")).some((el) => (el.textContent ?? "").trim() === "·")).toBe(false);
  });

  it("says who and how as one run of words with a space between them, and puts the score beside that run, as the design's chip does", () => {
    // The design draws `<span>name <span>· via Lanekeeper</span></span>` and the score at a gap of 8 px after it. Name and phrase as two
    // flex items at that gap are 4.6 px further apart than the design's space and wrap as two boxes in a narrow panel.
    const { container } = signedIn(<AttributionChip attribution={attribution()} />);
    const [words, score, ...rest] = Array.from(screen.getByRole("button").children);
    expect(rest).toHaveLength(0);
    expect(words).toHaveTextContent(/^You · via Lanekeeper$/);
    expect(score).toHaveAttribute("data-slot", "confidence");
    expect(container.querySelector("[data-slot='attribution'] button")).toBe(screen.getByRole("button"));
  });

  it("sets the space between the name and the words after it in the words' own weight, as the design does", () => {
    // The design's space is a text node of the outer span (weight 400), between the 500-weight name and the words. In the 500-weight run the
    // space is 0.16 px narrower, which moves the score and every glyph after it off the design's by that much (seen in the History panel
    // against the design's frame 1j: the words were 161.36 px wide, the design's 161.52).
    signedIn(<AttributionChip attribution={attribution()} />);
    const text = screen.getByRole("button").querySelector("[data-slot='attribution-text']");
    expect(text).not.toBeNull();
    const spaces = [...Array.from(text?.querySelectorAll("*") ?? []), ...(text === null ? [] : [text])].flatMap((element) =>
      Array.from(element.childNodes)
        .filter((node) => node.nodeType === 3 && node.textContent === " ")
        .map(() => element),
    );
    expect(spaces).toHaveLength(1);
    expect(spaces[0], "the space is not in a 500-weight run").not.toHaveClass("font-medium");
    // The name is: it is the part that is heavier.
    expect(screen.getByRole("button").querySelector(".font-medium")).toHaveTextContent(/^You$/);
  });

  it("does not say You when there is no session to say who You is", () => {
    render(<AttributionChip attribution={attribution()} />);
    expect(screen.queryByText("You")).not.toBeInTheDocument();
    expect(screen.getByRole("button")).toHaveTextContent("Lanekeeper user 00000000");
  });

  it("takes the signed-in person from `you` when the screen gives it, ahead of the session", () => {
    signedIn(<AttributionChip attribution={attribution()} you={OTHER} />);
    expect(screen.getByRole("button")).toHaveTextContent("Lanekeeper user 00000000");
    expect(screen.queryByText("You")).not.toBeInTheDocument();
  });

  it("names another person only by the lookup, and otherwise by the short id: never by a name it made up", () => {
    const { unmount } = signedIn(<AttributionChip attribution={attribution({ actor: { kind: "user", user: OTHER } })} />);
    expect(screen.getByRole("button")).toHaveTextContent("Lanekeeper user 3f2a91bc");
    unmount();
    signedIn(<AttributionChip attribution={attribution({ actor: { kind: "user", user: OTHER } })} nameOf={(id) => (id === OTHER ? "Marco Diaz" : undefined)} />);
    expect(screen.getByRole("button")).toHaveTextContent("Marco Diaz");
    expect(screen.getByRole("button")).not.toHaveTextContent("Lanekeeper user");
  });

  it.each([
    ["an edit on the NFS VM", attribution({ source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" } }), "dana.okoye", "edited on the NFS VM", "High"],
    ["a sync job", attribution({ source: "sync_job", confidence: "high", actor: { kind: "job", job: { name: "csp-tenant-data-sit7", uid: "u-42" } } }), "Sync job csp-tenant-data-sit7", "", "High"],
    ["an NFS client with nobody named", attribution({ source: "nfs_client", confidence: "medium", actor: null }), "NFS client", "(no sync running)", "Medium"],
    ["the owner of the file", attribution({ source: "fs_owner_hint", confidence: "low", actor: { kind: "os_login", login: "svc-batch" } }), "svc-batch", "(the file's owner, a hint)", "Low"],
  ] as const)("shows %s", (_name, value, who, how, word) => {
    render(<AttributionChip attribution={value} />);
    const chip = screen.getByRole("button");
    expect(chip).toHaveTextContent(who);
    if (how !== "") expect(chip).toHaveTextContent(how);
    expect(chip).toHaveTextContent(`Confidence: ${word}`);
  });

  it("shows an unknown author as such: dashed, no score, no person and no level", () => {
    const { container } = render(<AttributionChip attribution={attribution({ source: "unknown", confidence: "low", actor: null })} />);
    const root = container.querySelector("[data-slot='attribution']");
    expect(root).toHaveAttribute("data-known", "false");
    expect(root).toHaveAttribute("data-who", "unknown");
    expect(screen.getByRole("button")).toHaveTextContent("Unknown author");
    const meter = container.querySelector("[data-slot='confidence']");
    expect(meter).toHaveAttribute("data-level", "none");
    expect(meter?.className).toContain("border-dashed");
    // The contract wrote `low` for this one; the chip must not repeat it.
    expect(screen.getByRole("button")).not.toHaveTextContent("Low");
  });

  it("says that nothing is recorded when there is no attribution, and offers no evidence for it", () => {
    render(<AttributionChip attribution={null} />);
    expect(screen.getByText("Not recorded")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    render(<AttributionChip attribution={null} emptyLabel="No change recorded" />);
    expect(screen.getByText("No change recorded")).toBeInTheDocument();
  });

  it("is a control at least 24 px high, though the chip in it is 18 (WCAG 2.5.8, target size)", () => {
    render(<AttributionChip attribution={attribution()} />);
    expect(screen.getByRole("button").className).toMatch(/(^|\s)min-h-6(\s|$)/);
  });

  // A login, a job name or a user's name can be up to 256 characters, and the words after it ("(the file's owner, a hint)") are long too.
  // As separate items of a wrapping line they became three lines (the name, the words, the bars), in rows built for two: the chip ran
  // over the links under it and the group header above it (found by the axe and layout runs on a Drift row). The name and the words
  // are ONE run that gives way with an ellipsis, so the chip is that run and the bars, on one line or on two, never more.
  it("keeps the name and the words in one run that is cut with an ellipsis, so that the chip is never more than two lines", () => {
    render(<AttributionChip attribution={attribution({ source: "fs_owner_hint", confidence: "low", actor: { kind: "os_login", login: "a".repeat(256) } })} />);
    const button = screen.getByRole("button");
    const run = button.querySelector<HTMLElement>("[data-slot='attribution-text']");
    expect(run, "the name and the words are in one element").not.toBeNull();
    expect(run?.textContent).toContain("a".repeat(256));
    expect(run?.textContent).toContain("(the file's owner, a hint)");
    expect(run?.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(run?.className).toMatch(/(^|\s)max-w-full(\s|$)/);
    // The name has a width of its own to give way at, so a long one does not push the words out of sight.
    expect(run?.querySelector<HTMLElement>("[data-slot='safe-text']")?.className).toMatch(/(^|\s)max-w-\S+(\s|$)/);
    // Everything the chip says is still in its name, in order, for a screen reader.
    expect(button).toHaveAccessibleName(/^a{256} \(the file's owner, a hint\)\s*Confidence:\s*Low$/);
  });

  it("is plain text, not a button, when the evidence is turned off (a row that opens a drawer by itself)", () => {
    render(<AttributionChip attribution={attribution()} evidence={false} />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText("via Lanekeeper")).toBeInTheDocument();
  });

  it("has no axe violations in any state", async () => {
    render(
      <ul>
        <li>
          <AttributionChip attribution={attribution()} />
        </li>
        <li>
          <AttributionChip attribution={attribution({ source: "unknown", confidence: "low", actor: null })} />
        </li>
        <li>
          <AttributionChip attribution={null} />
        </li>
      </ul>,
    );
    await expectNoAxeViolations();
  });
});

describe("the evidence of a chip", () => {
  const rich = attribution({
    source: "sentinel_login",
    confidence: "high",
    actor: { kind: "os_login", login: "dana.okoye" },
    evidence: { executable: "vi", login_identity: "dokoye", observed_at: Date.UTC(2026, 9, 17, 13, 58, 20) },
  });

  it("opens on click as a dialog with the source, the confidence, who, and each piece of evidence", async () => {
    const user = userEvent.setup();
    render(<AttributionChip attribution={rich} />);
    await user.click(screen.getByRole("button"));
    const dialog = await screen.findByRole("dialog", { name: "Attribution evidence" });
    const text = dialog.textContent ?? "";
    for (const part of ["Source", "Edited on the NFS VM", "Confidence", "High", "OS Login user", "dana.okoye", "Executable", "vi", "Login identity", "dokoye", "Observed", "2026-10-17 13:58:20 UTC"]) expect(text).toContain(part);
    expect(text).toContain("never downgraded");
    expect(text).toContain("The sentinel on the NFS VM saw a login write the file.");
  });

  it("opens from the keyboard, closes with Escape and gives focus back to the chip", async () => {
    const user = userEvent.setup();
    render(<AttributionChip attribution={rich} />);
    await user.tab();
    const chip = screen.getByRole("button");
    expect(chip).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("dialog", { name: "Attribution evidence" })).toBeVisible();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(chip).toHaveFocus();
  });

  it("says that there is nothing more when no evidence was recorded, and never fills a row with a guess", async () => {
    const user = userEvent.setup();
    render(<AttributionChip attribution={attribution({ source: "unknown", confidence: "low", actor: null })} />);
    await user.click(screen.getByRole("button"));
    const dialog = await screen.findByRole("dialog", { name: "Attribution evidence" });
    expect(within(dialog).getByText("No further evidence was recorded.")).toBeInTheDocument();
    expect(within(dialog).getByText("Not identified")).toBeInTheDocument();
    expect(within(dialog).getByText("None")).toBeInTheDocument();
    expect(within(dialog).queryByText("Executable")).not.toBeInTheDocument();
    expect(within(dialog).queryByText("Observed")).not.toBeInTheDocument();
  });
});

describe("AttributionEvidence", () => {
  it("is a definition list that a drawer can show in place", async () => {
    const { container } = render(<AttributionEvidence attribution={attribution({ evidence: { executable: "vim" } })} you={ME} />);
    expect(container.querySelector("dl")).not.toBeNull();
    expect(container.querySelectorAll("dt")).toHaveLength(container.querySelectorAll("dd").length);
    expect(screen.getByText("Author").nextElementSibling).toHaveTextContent("You");
    expect(screen.getByText("Executable").nextElementSibling).toHaveTextContent("vim");
    expect(screen.queryByText("No further evidence was recorded.")).not.toBeInTheDocument();
    await expectNoAxeViolations();
  });
});

describe("AttributionSourceCell, the cell of an audit table", () => {
  it("shows the source and a quarter-step track with the level in words", () => {
    const { container } = render(<AttributionSourceCell attribution={attribution({ source: "sentinel_login", confidence: "medium", actor: { kind: "os_login", login: "dana" } })} />);
    expect(screen.getByText("Edited on the NFS VM")).toBeInTheDocument();
    const meter = container.querySelector("[data-slot='confidence']");
    expect(meter).toHaveAttribute("data-variant", "track");
    expect(meter).toHaveAttribute("data-step", "2");
    expect(meter).toHaveTextContent("Medium");
  });

  it("shows an unknown source in muted words with no score at all, as the design does (a Low next to Unknown would be a score that does not exist)", () => {
    const { container } = render(<AttributionSourceCell attribution={attribution({ source: "unknown", confidence: "low", actor: null })} />);
    expect(screen.getByText("Unknown").className).toContain("text-muted-foreground");
    expect(container.querySelector("[data-slot='confidence']")).toBeNull();
    expect(container).not.toHaveTextContent("Low");
  });

  it("shows a dash for an event with no attribution", () => {
    render(<AttributionSourceCell attribution={null} />);
    expect(screen.getByText("–")).toBeInTheDocument();
  });

  it("is plain text: it sits in a row that opens a drawer by itself", () => {
    render(<AttributionSourceCell attribution={attribution()} />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});

describe("UnknownAuthorBadge", () => {
  it("is a dashed, italic badge with an icon and the words unknown author: told apart from a person by shape and by words, not colour", () => {
    const { container } = render(<UnknownAuthorBadge />);
    const badge = container.firstElementChild as HTMLElement;
    expect(badge).toHaveAttribute("data-slot", "unknown-author");
    expect(badge).toHaveTextContent("unknown author");
    expect(badge.querySelector("svg")).not.toBeNull();
    expect(badge.className).toContain("border-dashed");
    expect(badge.className).toContain("italic");
  });
});
