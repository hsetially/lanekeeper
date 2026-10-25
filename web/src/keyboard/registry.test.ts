import { beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_REGISTRATIONS, SEQUENCE_MS, ShortcutRegistry, isInEditor, isTypingTarget, type Environment } from "./registry";

// The registry decides what a key press means. These tests drive it with real KeyboardEvents and a fake clock and focus,
// because the rules that matter (typing, dialogs, the editor, sequences, layers) are about what the event does and what the
// page then sees of it.

let clock = 0;
let active: Element | null = null;
let overlay = false;
const env: Environment = { now: () => clock, active: () => active, overlayOpen: () => overlay };

function press(registry: ShortcutRegistry, key: string, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  registry.handle(event);
  return event;
}

beforeEach(() => {
  clock = 1_000;
  active = null;
  overlay = false;
  document.body.innerHTML = "";
});

describe("a single key", () => {
  it("runs the handler of its row, and prevents the page from seeing the key", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("global.shortcuts-sheet", handler);
    const event = press(registry, "?", { shiftKey: true });
    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler.mock.calls[0]?.[0]).toMatchObject({ id: "global.shortcuts-sheet", binding: 0 });
    expect(event.defaultPrevented).toBe(true);
  });

  it("does nothing for a row nobody registered, and leaves the key to the page", () => {
    const registry = new ShortcutRegistry(env);
    const event = press(registry, "?", { shiftKey: true });
    expect(event.defaultPrevented).toBe(false);
  });

  it("stops firing once the handler is removed", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    const off = registry.register("global.shortcuts-sheet", handler);
    off();
    off();
    press(registry, "?", { shiftKey: true });
    expect(handler).not.toHaveBeenCalled();
    expect(registry.registered()).toEqual([]);
  });

  it("is ignored while a key is held down (a repeat), and for a bare modifier", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("global.shortcuts-sheet", handler);
    press(registry, "?", { shiftKey: true, repeat: true });
    press(registry, "Shift");
    expect(handler).not.toHaveBeenCalled();
  });

  it("is ignored while an input method is composing", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("global.shortcuts-sheet", handler);
    press(registry, "?", { shiftKey: true, isComposing: true });
    expect(handler).not.toHaveBeenCalled();
  });
});

describe("where a key fires", () => {
  it("does not fire a single key while a text field has focus, and does fire a chord there", () => {
    const registry = new ShortcutRegistry(env);
    const slash = vi.fn();
    const palette = vi.fn();
    registry.register("global.search-text-focus-query", slash);
    registry.register("global.command-palette", palette);
    const input = document.createElement("input");
    document.body.append(input);
    active = input;
    press(registry, "/");
    press(registry, "k", { ctrlKey: true });
    expect(slash).not.toHaveBeenCalled();
    expect(palette).toHaveBeenCalledTimes(1);
  });

  it("does not fire a single key in a textarea, a select or an editable element, but does in a button or a checkbox", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("global.shortcuts-sheet", handler);
    const make = (html: string): Element => {
      document.body.innerHTML = html;
      return document.body.firstElementChild as Element;
    };
    for (const html of ["<textarea></textarea>", "<select><option>a</option></select>", '<div contenteditable="true"></div>', '<div role="textbox"></div>', '<input type="search">']) {
      active = make(html);
      press(registry, "?", { shiftKey: true });
    }
    expect(handler).not.toHaveBeenCalled();
    for (const html of ["<button>b</button>", '<input type="checkbox">', "<a href='#'>l</a>"]) {
      active = make(html);
      press(registry, "?", { shiftKey: true });
    }
    expect(handler).toHaveBeenCalledTimes(3);
  });

  it("does not fire a single key while a dialog or a menu is open, and does not fire a chord either", () => {
    const registry = new ShortcutRegistry(env);
    const sheet = vi.fn();
    const palette = vi.fn();
    registry.register("global.shortcuts-sheet", sheet);
    registry.register("global.command-palette", palette);
    overlay = true;
    press(registry, "?", { shiftKey: true });
    press(registry, "k", { metaKey: true });
    expect(sheet).not.toHaveBeenCalled();
    expect(palette).not.toHaveBeenCalled();
  });

  it("fires a chord in the editor, and a single key not", () => {
    const registry = new ShortcutRegistry(env);
    const toggle = vi.fn();
    const sheet = vi.fn();
    registry.register("global.toggle-sidebar", toggle);
    registry.register("global.shortcuts-sheet", sheet);
    document.body.innerHTML = '<div class="monaco-editor"><textarea class="inputarea"></textarea></div>';
    active = document.querySelector("textarea");
    expect(isInEditor(active)).toBe(true);
    press(registry, "b", { ctrlKey: true });
    press(registry, "?", { shiftKey: true });
    expect(toggle).toHaveBeenCalledTimes(1);
    expect(sheet).not.toHaveBeenCalled();
  });

  it("lets a registration widen where its key fires", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("global.shortcuts-sheet", handler, { allow: { dialog: true, input: true } });
    const input = document.createElement("input");
    document.body.append(input);
    active = input;
    overlay = true;
    press(registry, "?", { shiftKey: true });
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("delivers Escape in a field, but not while a dialog is open (its own Esc closes it)", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("global.close-dialog-menu-or-palette", handler);
    const input = document.createElement("input");
    document.body.append(input);
    active = input;
    press(registry, "Escape");
    expect(handler).toHaveBeenCalledTimes(1);
    overlay = true;
    press(registry, "Escape");
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("classifies typing targets", () => {
    const el = (html: string): Element => {
      const host = document.createElement("div");
      host.innerHTML = html;
      return host.firstElementChild as Element;
    };
    expect(isTypingTarget(el("<input>"))).toBe(true);
    expect(isTypingTarget(el('<input type="button">'))).toBe(false);
    expect(isTypingTarget(el("<button></button>"))).toBe(false);
    expect(isTypingTarget(null)).toBe(false);
  });
});

describe("a sequence", () => {
  it("fires on the second key and names the binding that completed it", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("go-to.swimlanes-compare-feature-status-findings", handler);
    expect(press(registry, "g").defaultPrevented).toBe(true);
    expect(registry.pending).toBe(true);
    press(registry, "c");
    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler.mock.calls[0]?.[0]).toMatchObject({ binding: 1, action: "Compare" });
    expect(registry.pending).toBe(false);
  });

  it("each of the five second keys is its own binding", () => {
    const registry = new ShortcutRegistry(env);
    const seen: number[] = [];
    registry.register("go-to.swimlanes-compare-feature-status-findings", (hit) => {
      seen.push(hit.binding);
    });
    for (const second of ["s", "c", "f", "i", "a"]) {
      press(registry, "g");
      press(registry, second);
    }
    expect(seen).toEqual([0, 1, 2, 3, 4]);
  });

  it("times out after 1.2 seconds on the injected clock, and then the second key is an ordinary key", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("go-to.swimlanes-compare-feature-status-findings", handler);
    press(registry, "g");
    clock += SEQUENCE_MS + 1;
    const late = press(registry, "s");
    expect(handler).not.toHaveBeenCalled();
    expect(late.defaultPrevented).toBe(false);
  });

  it("still fires just inside the timeout", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("go-to.swimlanes-compare-feature-status-findings", handler);
    press(registry, "g");
    clock += SEQUENCE_MS;
    press(registry, "s");
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("is cancelled by a wrong second key, which is then handled as a fresh key", () => {
    const registry = new ShortcutRegistry(env);
    const goTo = vi.fn();
    const sheet = vi.fn();
    registry.register("go-to.swimlanes-compare-feature-status-findings", goTo);
    registry.register("global.shortcuts-sheet", sheet);
    press(registry, "g");
    press(registry, "?", { shiftKey: true });
    expect(goTo).not.toHaveBeenCalled();
    expect(sheet).toHaveBeenCalledTimes(1);
    expect(registry.pending).toBe(false);
  });

  it("is not started in a field, in the editor or in a dialog", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("go-to.swimlanes-compare-feature-status-findings", handler);
    const input = document.createElement("input");
    document.body.append(input);
    active = input;
    press(registry, "g");
    press(registry, "s");
    active = null;
    overlay = true;
    press(registry, "g");
    press(registry, "s");
    expect(handler).not.toHaveBeenCalled();
    expect(registry.pending).toBe(false);
  });

  it("is dropped when the dialog opens between the two keys", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("go-to.swimlanes-compare-feature-status-findings", handler);
    press(registry, "g");
    overlay = true;
    press(registry, "s");
    expect(handler).not.toHaveBeenCalled();
  });

  it("is dropped when its handler is removed while it waits", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    const off = registry.register("go-to.swimlanes-compare-feature-status-findings", handler);
    press(registry, "g");
    off();
    press(registry, "s");
    expect(handler).not.toHaveBeenCalled();
    expect(registry.pending).toBe(false);
  });

  it("ignores a Shift or Control press between the two keys", () => {
    const registry = new ShortcutRegistry(env);
    const handler = vi.fn();
    registry.register("go-to.swimlanes-compare-feature-status-findings", handler);
    press(registry, "g");
    press(registry, "Shift");
    press(registry, "s");
    expect(handler).toHaveBeenCalledTimes(1);
  });
});

describe("two handlers for one key", () => {
  it("asks the higher layer first even when the shell registered later, then the later registration", () => {
    const registry = new ShortcutRegistry(env);
    const calls: string[] = [];
    registry.register("local.filter.focus", () => {
      calls.push("screen-a");
    });
    registry.register("global.search-text-focus-query", () => {
      calls.push("shell");
    });
    press(registry, "/");
    expect(calls).toEqual(["screen-a"]);
    registry.register("local.filter.focus", () => {
      calls.push("screen-b");
    });
    calls.length = 0;
    press(registry, "/");
    expect(calls).toEqual(["screen-b"]);
  });

  it("falls through to the next handler when one declines, and to the page when all do", () => {
    const registry = new ShortcutRegistry(env);
    const calls: string[] = [];
    registry.register("global.search-text-focus-query", () => {
      calls.push("shell");
    });
    registry.register("local.filter.focus", () => {
      calls.push("screen");
      return false;
    });
    expect(press(registry, "/").defaultPrevented).toBe(true);
    expect(calls).toEqual(["screen", "shell"]);
    const lone = new ShortcutRegistry(env);
    lone.register("local.filter.focus", () => false);
    expect(press(lone, "/").defaultPrevented).toBe(false);
  });
});

describe("registration", () => {
  it("refuses an id that is not a row, a type-ahead row and a row with no key", () => {
    const registry = new ShortcutRegistry(env);
    expect(() => registry.register("no.such.row" as never, () => undefined)).toThrow(/no shortcut row/);
    expect(() => registry.register("tree-only.jump-to-the-next-row", () => undefined)).toThrow(/no key to bind/);
    expect(() => registry.register("search.toggle-buttons-no-shortcut-assigned", () => undefined)).toThrow(/no key to bind/);
  });

  it("is bounded", () => {
    const registry = new ShortcutRegistry(env);
    for (let i = 0; i < MAX_REGISTRATIONS; i++) registry.register("local.filter.focus", () => undefined);
    expect(() => registry.register("local.filter.focus", () => undefined)).toThrow(/too many/);
  });
});
