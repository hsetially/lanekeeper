import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { PickupState } from "@/api/generated/model";
import { service } from "@/mocks/design/swimlanes/fixtures";
import { FilePickup } from "./FilePickup";
import { ServicePickup } from "./ServicePickup";
import { pickupRollups, type PickupNodeLike } from "./rollup";
import { TreePickup } from "./TreePickup";

// Hostile text (S8, S12). A file path the hub says a service waits on comes from a person's NFS share: a right-to-left override
// must not reorder what is around it, a zero-width character must not make two names look the same, and markup must stay text.
// The states and flags are checked in components/pickup; here are the paths, in the text, in the title attribute and in the
// tooltip, and the states on the rows of a tree.

const BIDI = ["‪", "‫", "‬", "‭", "‮", "⁦", "⁧", "⁨", "⁩"];
const INVISIBLE = ["​", "‌", "‍", "‎", "‏", "﻿", "؜", "­", "⁠", " ", " ", "\u0007", "\u007F", "ㅤ"];
const hex = (ch: string): string => `U+${(ch.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}`;
const UNSAFE = /[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}]/u;
const NOW = Date.UTC(2026, 9, 17, 14, 0, 0);

/** Every character the browser would draw as itself or not at all, in any text node or attribute under `root`. */
function rawUnsafe(root: ParentNode): string[] {
  const found: string[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    if (node.nodeType === Node.TEXT_NODE) {
      for (const ch of node.textContent ?? "") if (ch !== "\t" && ch !== "\n" && UNSAFE.test(ch)) found.push(`text ${hex(ch)}`);
    } else {
      for (const attr of Array.from((node as Element).attributes)) for (const ch of attr.value) if (ch !== "\t" && ch !== "\n" && UNSAFE.test(ch)) found.push(`@${attr.name} ${hex(ch)}`);
    }
  }
  return found;
}

const draw = (files: string[]) => render(<ServicePickup service={service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: NOW - 60_000, pending_files: files })} now={NOW} />);

describe("the file a service waits on, with text it cannot trust", () => {
  it.each([...BIDI, ...INVISIBLE])("shows %j in a path as a marker, never as itself (in the text or in the title)", (ch) => {
    const { container } = draw([`tx-infinity-api/ap${ch}p.yml`, `other/b${ch}.yml`]);
    expect(rawUnsafe(container)).toEqual([]);
    expect(container.textContent).toContain(`<${hex(ch)}>`);
    expect(screen.getByRole("img", { name: new RegExp(hex(ch).replace("+", "\\+")) })).toBeInTheDocument();
    expect(container.querySelector("[title*='Waiting']")).toBeNull();
    const title = container.querySelector("span[title]:not([title^='since'])")?.getAttribute("title") ?? "";
    expect(title).toContain(`<${hex(ch)}>`);
  });

  it("shows the classic spoof for what it is", () => {
    const { container } = draw(["invoice‮gnp.exe"]);
    expect(container.textContent).toContain("invoice<U+202E>gnp.exe");
    expect(container.querySelector("bdi")).not.toBeNull();
  });

  it.each(["<img src=x onerror=alert(1)>", "<script>alert(1)</script>", "\"><svg onload=alert(1)>", "javascript:alert(1)", "{{constructor.constructor('alert(1)')()}}"])("keeps %s as text: no element, no handler, no link", (path) => {
    const { container } = draw([path, path]);
    expect(container.textContent).toContain(path.split("/").pop());
    expect(container.querySelector("img, script, iframe, object, embed, a[href], form, input, button")).toBeNull();
    for (const el of container.querySelectorAll("*")) for (const attr of Array.from(el.attributes)) expect(attr.name.startsWith("on"), `${attr.name} on <${el.tagName.toLowerCase()}>`).toBe(false);
  });

  it("keeps a 1,024-character path in one line that gives way, and the title to twenty paths", () => {
    const long = `${"d/".repeat(500)}x.yml`;
    const { container } = draw(Array.from({ length: 30 }, () => long));
    const line = container.querySelector("span[title]:not([title^='since'])");
    expect(line?.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect((line?.getAttribute("title") ?? "").split("\n")).toHaveLength(21);
  });
});

describe("a tree row with a state it cannot trust", () => {
  const file = (state: unknown): PickupNodeLike => ({ id: "api/a.yml", isDir: false, lazy: false, more: null, entry: { pickup_state: state as PickupState }, children: [] });

  it.each(["<img src=x onerror=alert(1)>", "pending_restart", "‮live", ""])("draws %j as 'Pickup unknown' and never repeats it", (hostile) => {
    const { container } = render(<TreePickup node={file(hostile)} rollup={undefined} />);
    expect(screen.getByRole("img", { name: "Pickup unknown" })).toBeInTheDocument();
    expect(container.innerHTML).not.toContain("onerror");
    expect(container.textContent).not.toContain("pending_restart");
  });

  it("counts a folder's files without trusting the states: an unknown state is not stated", () => {
    const dir: PickupNodeLike = { id: "api", isDir: true, lazy: false, more: null, entry: { pickup_state: null }, children: [file("<b>x</b>"), file("needs_notify_or_restart")] };
    render(<TreePickup node={dir} rollup={pickupRollups([dir]).get("api")} />);
    expect(screen.getByRole("img", { name: "1 Needs notify or restart" })).toBeInTheDocument();
  });

  it("draws the head of a file with an unknown state as 'Pickup unknown'", () => {
    const { container } = render(<FilePickup entry={{ pickup_state: "<script>alert(1)</script>" as PickupState }} />);
    expect(container.textContent).toContain("Pickup unknown");
    expect(container.querySelector("script")).toBeNull();
  });
});
