import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { NotificationsNote } from "./NotificationsNote";

// The notifications flag of a service, as the hub reported it (Q35: do services subscribe? is open). The note says on or off and
// that the hub said so; it does not say what the flag does.

const note = (container: HTMLElement): HTMLElement => {
  const el = container.querySelector<HTMLElement>("[data-slot='notifications-note']");
  if (el === null) throw new Error("no note");
  return el;
};

describe("the notifications note", () => {
  it("says 'on' as the hub reported it", () => {
    const { container } = render(<NotificationsNote enabled={true} />);
    expect(note(container).textContent).toBe("Notifications: on (as reported by the hub)");
    expect(note(container)).toHaveAttribute("data-notifications", "on");
  });

  it("says 'off' as the hub reported it", () => {
    const { container } = render(<NotificationsNote enabled={false} />);
    expect(note(container).textContent).toBe("Notifications: off (as reported by the hub)");
    expect(note(container)).toHaveAttribute("data-notifications", "off");
  });

  it("says the hub did not report it when it is neither yes nor no", () => {
    for (const bad of [null, undefined, "yes", 1] as unknown[]) {
      const { container, unmount } = render(<NotificationsNote enabled={bad as boolean} />);
      expect(note(container).textContent).toBe("Notifications: not reported by the hub");
      expect(note(container)).toHaveAttribute("data-notifications", "unreported");
      unmount();
    }
  });

  it("claims nothing about what the flag does", () => {
    for (const enabled of [true, false]) {
      const { container, unmount } = render(<NotificationsNote enabled={enabled} />);
      expect(note(container).textContent).not.toMatch(/subscrib|listen|receiv|work|will|can|enabled|disabled/i);
      unmount();
    }
  });

  it("draws its icon for decoration only, so that the words are the whole of it", () => {
    const { container } = render(<NotificationsNote enabled={true} />);
    const icon = note(container).querySelector("svg");
    expect(icon).not.toBeNull();
    expect(icon).toHaveAttribute("aria-hidden", "true");
  });
});
