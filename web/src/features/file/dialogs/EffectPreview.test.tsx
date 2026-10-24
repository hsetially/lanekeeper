import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ChangePreview, PreviewedChange } from "@/api/generated/model";
import { EffectPreview } from "./EffectPreview";

// What an upload or a delete would do, from the engine, before anybody confirms (D46, D83, D85): who reads the file, what
// would differ in the effective config, and the consequences worth a look. Values are masked by the server where the secret scan
// flagged them; the web only words and lays out what it was told.

const value = (text: string) => ({ text, ty: "string" as const, flag: null });
const change = (path: string, kind: "added" | "removed" | "changed", left: string | null, right: string | null, flagged = false): PreviewedChange => ({
  change: { path, kind, left: left === null ? null : value(left), right: right === null ? null : value(right), left_location: null, right_location: null, severity: null },
  flagged,
});
const preview = (over: Partial<ChangePreview> = {}): ChangePreview => ({
  preview_id: "prev-1",
  expires_at: Date.UTC(2026, 9, 17, 10, 52),
  scope: { kind: "every_service" },
  warnings: [],
  changes: [change("tx.core.pool.maxSize", "changed", "40", "25"), change("tx.core.features.fastSettle", "removed", "true", null), change("tx.core.metrics.enabled", "added", null, "true")],
  changes_total: 3,
  effective_changed: true,
  ...over,
});

describe("EffectPreview", () => {
  it("lists what would change in the effective config, old value against new (2a)", () => {
    render(<EffectPreview preview={preview()} />);
    const table = screen.getByRole("table", { name: "What changes in the effective config" });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(4);
    expect(rows[1]).toHaveTextContent("Changed");
    expect(rows[1]).toHaveTextContent("tx.core.pool.maxSize");
    expect(rows[1]).toHaveTextContent("40");
    expect(rows[1]).toHaveTextContent("25");
    expect(rows[2]).toHaveTextContent("Removed");
    expect(rows[2]).toHaveTextContent("not set");
    expect(rows[3]).toHaveTextContent("Added");
  });

  it("says how many more there are than it lists", () => {
    render(<EffectPreview preview={preview({ changes_total: 53 })} />);
    expect(screen.getByText("and 50 more changes are not listed.")).toBeInTheDocument();
  });

  it("says when nothing changes in the effective config", () => {
    render(<EffectPreview preview={preview({ changes: [], changes_total: 0, effective_changed: false })} />);
    expect(screen.getByText("The effective config does not change.")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("says who reads the file", () => {
    const { rerender } = render(<EffectPreview preview={preview()} />);
    expect(screen.getByText("Every service reads this file.")).toBeInTheDocument();
    rerender(<EffectPreview preview={preview({ scope: { kind: "application", app: "tx-infinity-core" } })} />);
    expect(screen.getByText("The application tx-infinity-core reads this file.")).toBeInTheDocument();
    rerender(<EffectPreview preview={preview({ scope: { kind: "folder", path: "tx-infinity-api" } })} />);
    expect(screen.getByText("Services that read the folder tx-infinity-api read this file.")).toBeInTheDocument();
  });

  it.each([
    [{ kind: "falls_back_to_base" }, "This tenant file is removed, so the services fall back to the base file."],
    [{ kind: "no_file_left" }, "No file is left for these services: nothing will define these settings."],
    [{ kind: "forks_from_base" }, "This creates a tenant file that forks from the base file: later base changes will not reach it."],
    [{ kind: "needs_notify_or_restart" }, "Services pick this change up after a notify or a restart."],
    [{ kind: "new_folder_needs_config_server_restart" }, "A new folder needs the config server to restart before it is read."],
    [{ kind: "channels_yml_needs_config_server_restart" }, "channels.yml is read when the config server starts: it needs a restart."],
    [{ kind: "never_read", suffix: "sit9" }, 'No swimlane reads a file with the suffix "sit9": this file would never be used.'],
  ] as const)("words the warning %j", (warning, text) => {
    render(<EffectPreview preview={preview({ warnings: [warning] })} />);
    expect(screen.getByText(text)).toBeInTheDocument();
  });

  it("marks a danger as a danger, with its icon and words, not colour alone", () => {
    render(<EffectPreview preview={preview({ warnings: [{ kind: "no_file_left" }] })} />);
    expect(screen.getByRole("alert")).toHaveTextContent("No file is left for these services");
  });

  it("shows a masked value as the marker the server sent, and says it is masked", () => {
    render(<EffectPreview preview={preview({ changes: [change("k.secret", "changed", "[REDACTED: 12 chars, aws_key]", "[REDACTED: 9 chars, aws_key]", true)], changes_total: 1 })} />);
    expect(screen.getByText("[REDACTED: 12 chars, aws_key]")).toBeInTheDocument();
    expect(screen.getByText("Masked")).toBeInTheDocument();
  });

  it("says until when the preview is good: the upload or delete is refused after that", () => {
    render(<EffectPreview preview={preview()} />);
    expect(screen.getByText("This preview is good until 2026-10-17 10:52 UTC.")).toBeInTheDocument();
  });

  it("draws values and keys as data", () => {
    render(<EffectPreview preview={preview({ changes: [change("a‮b", "added", null, "x​y")], changes_total: 1 })} />);
    expect(screen.getByText("<U+202E>")).toBeInTheDocument();
    expect(screen.getByText("<U+200B>")).toBeInTheDocument();
  });
});
