import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { FileFormat, ValidationIssue } from "@/api/generated/model";
import { fakeMonaco } from "../fake-monaco";
import { renderFile } from "../test-utils";
import { EditPane, type EditPaneProps } from "./EditPane";

// Edit mode as the design draws it (1g to 1i): the editor, a Problems panel when the server refused the content, the status
// bar, and the bar with Cancel and Save. Save is never silent: it opens the review (the page does), and it says "Submit for
// approval" for a person whose writes need one.

const format: FileFormat = { eol: "crlf", encoding: "utf8", bom: false };
const allowed = { allowed: true, reason: null };

function setup(over: Partial<EditPaneProps> = {}) {
  const m = fakeMonaco();
  const props: EditPaneProps = {
    name: "tx-infinity-core-sit7.yml",
    lane: "sitb",
    initialText: "a: 1\r\nb: 2\r\n",
    text: "a: 1\r\nb: 2\r\n",
    format,
    dirty: false,
    issues: [],
    approval: false,
    saveGate: allowed,
    rebasedFrom: null,
    onText: vi.fn(),
    onCancel: vi.fn(),
    onSave: vi.fn(),
    load: () => Promise.resolve(m.api),
    ...over,
  };
  return { m, props, ...renderFile(<EditPane {...props} />) };
}

describe("EditPane", () => {
  it("shows the editor, the status bar and the bar with Cancel and Save to the swimlane's NFS", async () => {
    const { m } = setup();
    await waitFor(() => expect(m.monaco.editor.create).toHaveBeenCalled());
    const status = await screen.findByRole("group", { name: "File format" });
    expect(status).toHaveTextContent("CRLF");
    expect(status).toHaveTextContent("UTF-8");
    expect(status).toHaveTextContent("YAML");
    expect(status).toHaveTextContent("2 lines");
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save to sitb NFS" })).toBeInTheDocument();
  });

  it("keeps Save off until something has changed, and says why", async () => {
    setup();
    const save = await screen.findByRole("button", { name: "Save to sitb NFS" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    expect(save).toHaveAccessibleDescription("No changes to save");
    expect(screen.getByText("No changes yet")).toBeInTheDocument();
  });

  it("offers Save once there are changes, and asks the page to review it", async () => {
    const { props } = setup({ dirty: true, text: "a: 2\r\n" });
    expect(await screen.findByText("Unsaved changes")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Save to sitb NFS" }));
    expect(props.onSave).toHaveBeenCalledTimes(1);
  });

  it("counts the lines of what is being edited, not of what it started as", async () => {
    setup({ dirty: true, text: "a\nb\nc\nd\n" });
    expect(await screen.findByRole("group", { name: "File format" })).toHaveTextContent("4 lines");
  });

  it("says Submit for approval, and who must approve, for a person whose writes need it (1i)", async () => {
    setup({ approval: true, dirty: true, text: "x" });
    expect(await screen.findByRole("button", { name: "Submit for approval" })).toBeInTheDocument();
    expect(screen.getByText("Needs approval from an Admin")).toBeInTheDocument();
  });

  it("disables Save with the reason it is given (a Viewer, an agent that is offline)", async () => {
    setup({ dirty: true, text: "x", saveGate: { allowed: false, reason: "Unavailable while the agent is offline" } });
    const save = await screen.findByRole("button", { name: "Save to sitb NFS" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    expect(save).toHaveAccessibleDescription("Unavailable while the agent is offline");
  });

  it("cancels", async () => {
    const { props } = setup();
    await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(props.onCancel).toHaveBeenCalledTimes(1);
  });

  it("reports what is typed", async () => {
    const { m, props } = setup();
    await waitFor(() => expect(m.monaco.editor.create).toHaveBeenCalled());
    act(() => m.model.setValue("typed"));
    expect(props.onText).toHaveBeenCalledWith("typed");
  });

  describe("when the server refused the content", () => {
    const secret: ValidationIssue = { kind: "secret_match", line: 120, rule: "private_key" };
    const dup: ValidationIssue = { kind: "duplicate_key", line: 90 };

    it("lists the problems with their lines, an error before a warning, in the app's own words (1g)", async () => {
      setup({ dirty: true, text: "x", issues: [dup, secret] });
      const panel = await screen.findByRole("region", { name: "Problems" });
      const rows = within(panel).getAllByRole("listitem");
      expect(rows).toHaveLength(2);
      expect(rows[0]).toHaveTextContent("Error");
      expect(rows[0]).toHaveTextContent("Line 120 looks like a secret (private_key). Save is blocked.");
      expect(rows[0]).toHaveTextContent("L120");
      expect(rows[1]).toHaveTextContent("Warning");
      expect(within(panel).getByText("2")).toBeInTheDocument();
    });

    it("blocks the save with a line number and never shows the flagged text (1h)", async () => {
      setup({ dirty: true, text: "x", issues: [secret] });
      const alert = await screen.findByRole("alert");
      expect(alert).toHaveTextContent("Save blocked: line 120 looks like a secret (private_key)");
      expect(alert).toHaveTextContent("Lanekeeper never shows the flagged text");
    });

    it("marks the lines in the editor", async () => {
      const { m } = setup({ dirty: true, text: "x", issues: [secret, dup] });
      await waitFor(() => expect(m.monaco.editor.setModelMarkers).toHaveBeenCalled());
      const markers = (m.monaco.editor.setModelMarkers.mock.calls.at(-1) as unknown as [unknown, string, Array<{ severity: number; message: string }>])[2];
      expect(markers.map((x) => x.severity).sort()).toEqual([4, 8]);
    });

    it("does not show a Problems panel or an alert when there is nothing to show", async () => {
      setup({ dirty: true, text: "x" });
      await screen.findByRole("button", { name: "Save to sitb NFS" });
      expect(screen.queryByRole("region", { name: "Problems" })).not.toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });
  });

  it("says what the next save is based on when the person chose to re-apply on top of a newer version", async () => {
    setup({ dirty: true, text: "x", rebasedFrom: "a3f9c21" + "0".repeat(57) });
    expect(await screen.findByText(/Based on the version that is on NFS now/)).toBeInTheDocument();
  });
});
