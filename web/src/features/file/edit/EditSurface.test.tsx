import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { EDITOR_OPTIONS } from "@/editor/loader";
import { fakeMonaco } from "../fake-monaco";
import { EditSurface } from "./EditSurface";

// Edit mode's surface: Monaco, loaded lazily through src/editor (never imported here), made into an editor on a box that the
// page sizes. It reports what is typed and nothing else leaves it. Monaco does not run in jsdom, so a fake of the few calls
// the surface makes stands in; the real one is measured under the production CSP by e2e/editor/csp.spec.ts.

const props = { label: "tx-infinity-core-sit7.yml", initialText: "a: 1\n", language: "yaml" as const, markers: [], onTextChange: () => undefined };

describe("EditSurface", () => {
  it("says it is loading the editor while the chunk loads", () => {
    render(<EditSurface {...props} load={() => new Promise(() => undefined)} />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading the editor");
  });

  it("makes an editor on the file's text, in the file's language, with the app's options", async () => {
    const m = fakeMonaco();
    render(<EditSurface {...props} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.create).toHaveBeenCalledTimes(1));
    expect(m.monaco.editor.createModel).toHaveBeenCalledWith("a: 1\n", "yaml");
    const options = (m.monaco.editor.create.mock.calls[0] as unknown as [HTMLElement, Record<string, unknown>])[1];
    expect(options).toMatchObject({ ...EDITOR_OPTIONS, readOnly: false });
    expect(options["ariaLabel"]).toBe("Editor for tx-infinity-core-sit7.yml");
    expect(m.editor.focus).toHaveBeenCalled();
  });

  it("reports what is typed", async () => {
    const m = fakeMonaco();
    const onTextChange = vi.fn();
    render(<EditSurface {...props} onTextChange={onTextChange} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.create).toHaveBeenCalled());
    act(() => m.model.setValue("a: 2\n"));
    expect(onTextChange).toHaveBeenCalledWith("a: 2\n");
  });

  it("does not make a new editor when the page re-renders with the same text, and keeps what was typed", async () => {
    const m = fakeMonaco();
    const { rerender } = render(<EditSurface {...props} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.create).toHaveBeenCalled());
    act(() => m.model.setValue("typed\n"));
    rerender(<EditSurface {...props} markers={[]} load={() => Promise.resolve(m.api)} />);
    expect(m.monaco.editor.create).toHaveBeenCalledTimes(1);
    expect(m.model.getValue()).toBe("typed\n");
  });

  it("lays the editor out again when its box changes size", async () => {
    const m = fakeMonaco();
    render(<EditSurface {...props} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.editor.layout).toHaveBeenCalled());
  });

  it("marks the lines the server refused, with the app's own words", async () => {
    const m = fakeMonaco();
    render(<EditSurface {...props} markers={[{ line: 1, severity: "error", message: "Line 1 looks like a secret (private_key). Save is blocked." }, { line: 9, severity: "warning", message: "Line 9 repeats a key that is set earlier in the file." }]} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.setModelMarkers).toHaveBeenCalled());
    const [, owner, markers] = m.monaco.editor.setModelMarkers.mock.calls.at(-1) as unknown as [unknown, string, Array<Record<string, unknown>>];
    expect(owner).toBe("lanekeeper");
    expect(markers).toEqual([
      { severity: 8, startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 40, message: "Line 1 looks like a secret (private_key). Save is blocked." },
      // Line 9 is past the end of the text (two lines to the editor: "a: 1" and the empty one after the final line ending): it is
      // marked on the last line rather than not at all.
      { severity: 4, startLineNumber: 2, startColumn: 1, endLineNumber: 2, endColumn: 40, message: "Line 9 repeats a key that is set earlier in the file." },
    ]);
  });

  it("clears the marks when there are none", async () => {
    const m = fakeMonaco();
    const { rerender } = render(<EditSurface {...props} markers={[{ line: 1, severity: "error", message: "m" }]} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.setModelMarkers).toHaveBeenCalled());
    rerender(<EditSurface {...props} markers={[]} load={() => Promise.resolve(m.api)} />);
    expect((m.monaco.editor.setModelMarkers.mock.calls.at(-1) as unknown as [unknown, string, unknown[]])[2]).toEqual([]);
  });

  it("follows the app's theme", async () => {
    document.documentElement.classList.add("dark");
    const m = fakeMonaco();
    render(<EditSurface {...props} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.setTheme).toHaveBeenCalledWith("lk-dark"));
    act(() => document.documentElement.classList.remove("dark"));
    await waitFor(() => expect(m.monaco.editor.setTheme).toHaveBeenLastCalledWith("lk-light"));
  });

  it("draws a comment in the dark theme in a green that reads at 4.5:1 on the editor's background (axe `color-contrast` found Monaco's own #608B4E at 4.2:1)", async () => {
    const m = fakeMonaco();
    render(<EditSurface {...props} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.defineTheme).toHaveBeenCalled());
    const calls = m.monaco.editor.defineTheme.mock.calls as unknown as Array<[string, { base: string; inherit: boolean; rules: Array<{ token: string; foreground?: string }> }]>;
    const [name, theme] = calls.find(([n]) => n === "lk-dark") ?? ["", { base: "", inherit: false, rules: [] }];
    expect(name).toBe("lk-dark");
    expect(theme.base).toBe("vs-dark");
    expect(theme.inherit).toBe(true);
    const comment = theme.rules.find((r) => r.token === "comment")?.foreground ?? "";
    const luminance = (hex: string): number => {
      const [r, g, b] = [0, 2, 4].map((i) => {
        const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
        return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
    };
    // vs-dark paints the editor on #1E1E1E.
    const ratio = (luminance(comment) + 0.05) / (luminance("1E1E1E") + 0.05);
    expect(ratio).toBeGreaterThanOrEqual(4.5);
  });

  it("keeps the line that sticks to the top of the editor readable while the pointer is over it, in the light theme (axe `color-contrast` found #008080 on Monaco's #F0F0F0 at 4.18:1)", async () => {
    const m = fakeMonaco();
    render(<EditSurface {...props} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.defineTheme).toHaveBeenCalled());
    const calls = m.monaco.editor.defineTheme.mock.calls as unknown as Array<[string, { base: string; inherit: boolean; colors: Record<string, string> }]>;
    const light = calls.find(([n]) => n === "lk-light");
    expect(light?.[1].base).toBe("vs");
    expect(light?.[1].inherit).toBe(true);
    const hover = (light?.[1].colors["editorStickyScrollHover.background"] ?? "").replace("#", "");
    const luminance = (hex: string): number => {
      const [r, g, b] = [0, 2, 4].map((i) => {
        const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
        return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
    };
    expect(hover).toMatch(/^[0-9a-fA-F]{6}$/);
    // The colours of `vs` that read on the editor's white: the key of a mapping (teal, the token of `zone-118:`) and a comment (green).
    for (const token of ["008080", "008000"]) expect((luminance("FFFFFF") + 0.05) / (luminance(token) + 0.05), `${token} on the editor's background`).toBeGreaterThanOrEqual(4.5);
    for (const token of ["008080", "008000"]) expect((luminance(hover) + 0.05) / (luminance(token) + 0.05), `${token} on the hovered sticky line`).toBeGreaterThanOrEqual(4.5);
  });

  it("lets go of the editor and the model when it goes", async () => {
    const m = fakeMonaco();
    const { unmount } = render(<EditSurface {...props} load={() => Promise.resolve(m.api)} />);
    await waitFor(() => expect(m.monaco.editor.create).toHaveBeenCalled());
    unmount();
    expect(m.editor.dispose).toHaveBeenCalledTimes(1);
    expect(m.model.dispose).toHaveBeenCalledTimes(1);
    expect(m.listeners.size).toBe(0);
  });

  it("falls back to the file, read-only, with an alert, when the editor cannot be loaded; and tries again on request", async () => {
    const m = fakeMonaco();
    const load = vi.fn().mockRejectedValueOnce(new TypeError("Failed to fetch dynamically imported module")).mockResolvedValueOnce(m.api);
    render(<EditSurface {...props} load={load} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("The editor could not be loaded");
    expect(screen.getByRole("table", { name: "tx-infinity-core-sit7.yml" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(m.monaco.editor.create).toHaveBeenCalled());
    expect(load).toHaveBeenCalledTimes(2);
  });
});
