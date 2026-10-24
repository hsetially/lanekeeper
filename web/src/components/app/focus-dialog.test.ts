import { afterEach, describe, expect, it, vi } from "vitest";
import { productSources } from "@/test/source-files";
import { focusDialog } from "./focus-dialog";

// The outcome and problem dialogs both start with focus on the dialog itself, not on the first control in it.

afterEach(() => {
  document.body.innerHTML = "";
});

describe("focusDialog", () => {
  it("keeps the library from moving focus to the first control, and focuses the dialog itself", () => {
    document.body.innerHTML = '<div id="d" role="dialog" tabindex="-1"><button id="first">first</button></div>';
    const dialog = document.getElementById("d") as HTMLElement;
    const event = new Event("focus", { cancelable: true });
    Object.defineProperty(event, "target", { value: dialog });
    const prevent = vi.spyOn(event, "preventDefault");
    focusDialog(event);
    expect(prevent).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(dialog);
  });

  it("leaves an event whose target is not an element alone", () => {
    const event = new Event("focus", { cancelable: true });
    const prevent = vi.spyOn(event, "preventDefault");
    focusDialog(event);
    expect(prevent).not.toHaveBeenCalled();
  });

  it("is written once: the problem dialog and the outcome dialog share it", () => {
    const holders = productSources().filter(({ text }) => /function focusDialog\b/.test(text)).map((f) => f.path);
    expect(holders).toEqual(["src/components/app/focus-dialog.ts"]);
  });
});
