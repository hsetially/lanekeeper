import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderFile } from "../test-utils";
import { LineEndingNotice } from "./LineEndingNotice";

// Two files whose text is the same and whose line endings are not (Diff, 1c). The diff would be every line of the file, so it
// is hidden behind this notice; the person can still see it, or convert the NFS file.

const allowed = { allowed: true, reason: null };
const props = { nfsEol: "crlf" as const, lines: 214, against: "Git", canShowAnyway: true, convertGate: allowed, onShowAnyway: vi.fn(), onConvert: vi.fn() };

describe("LineEndingNotice", () => {
  it("says only the line endings differ, and which each side uses (1c)", async () => {
    renderFile(<LineEndingNotice {...props} />);
    expect(await screen.findByText("Only line endings differ")).toBeInTheDocument();
    expect(screen.getByText(/NFS uses/)).toHaveTextContent("NFS uses CRLF and Git uses LF. All 214 lines have the same text, so the diff is hidden. This is why the file shows NFS ahead.");
  });

  it("reads the other way round for an LF file", async () => {
    renderFile(<LineEndingNotice {...props} nfsEol="lf" />);
    expect(await screen.findByText(/NFS uses/)).toHaveTextContent("NFS uses LF and Git uses CRLF.");
  });

  it("does not guess the other side's endings when NFS has mixed ones", async () => {
    renderFile(<LineEndingNotice {...props} nfsEol="mixed" />);
    const text = (await screen.findByText(/NFS uses/)).textContent ?? "";
    expect(text).toContain("NFS uses mixed line endings.");
    expect(text).not.toContain("Git uses");
  });

  it("is not about Git when the other side is another version", async () => {
    renderFile(<LineEndingNotice {...props} against="sitc" />);
    const text = (await screen.findByText(/NFS uses/)).textContent ?? "";
    expect(text).toContain("sitc uses LF");
    expect(text).not.toContain("shows NFS ahead");
  });

  it("shows the diff anyway on request", async () => {
    const onShowAnyway = vi.fn();
    renderFile(<LineEndingNotice {...props} onShowAnyway={onShowAnyway} />);
    await userEvent.click(await screen.findByRole("button", { name: "Show diff anyway" }));
    expect(onShowAnyway).toHaveBeenCalledTimes(1);
  });

  it("has nothing to show anyway when the server sent no lines for it", async () => {
    renderFile(<LineEndingNotice {...props} canShowAnyway={false} />);
    await screen.findByText("Only line endings differ");
    expect(screen.queryByRole("button", { name: "Show diff anyway" })).not.toBeInTheDocument();
  });

  it("converts NFS to the other ending, naming it", async () => {
    const onConvert = vi.fn();
    renderFile(<LineEndingNotice {...props} onConvert={onConvert} />);
    await userEvent.click(await screen.findByRole("button", { name: "Convert NFS file to LF" }));
    expect(onConvert).toHaveBeenCalledWith("lf");
  });

  it("converts an LF file to CRLF, and a mixed one to LF", async () => {
    const onConvert = vi.fn();
    const { unmount } = renderFile(<LineEndingNotice {...props} nfsEol="lf" onConvert={onConvert} />);
    await userEvent.click(await screen.findByRole("button", { name: "Convert NFS file to CRLF" }));
    expect(onConvert).toHaveBeenLastCalledWith("crlf");
    unmount();
    renderFile(<LineEndingNotice {...props} nfsEol="mixed" onConvert={onConvert} />);
    await userEvent.click(await screen.findByRole("button", { name: "Convert NFS file to LF" }));
    expect(onConvert).toHaveBeenLastCalledWith("lf");
  });

  it("disables the conversion with its reason when the person may not write", async () => {
    renderFile(<LineEndingNotice {...props} convertGate={{ allowed: false, reason: "Needs the Editor role. Ask an Admin to grant it." }} />);
    expect(await screen.findByRole("button", { name: "Convert NFS file to LF" })).toHaveAttribute("aria-disabled", "true");
  });
});
