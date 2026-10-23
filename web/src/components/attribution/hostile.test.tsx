import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Attribution } from "@/api/generated/model";
import { AttributionChip } from "./AttributionChip";
import { AttributionEvidence } from "./AttributionEvidence";
import { AttributionSourceCell } from "./AttributionSourceCell";

// Hostile text (S8, W21). An OS login, a job name, an executable and a person's name come from an NFS VM, a cluster or a
// directory, so they are data and never trusted: a right-to-left override must not reorder what is around it, a zero-width
// character must not make two names look the same, and markup must stay text. Everything the attribution components draw from
// them goes through SafeText.

const BIDI = ["\u202a", "\u202b", "\u202c", "\u202d", "\u202e", "\u2066", "\u2067", "\u2068", "\u2069"];
const INVISIBLE = ["\u200b", "\u200c", "\u200d", "\u200e", "\u200f", "\ufeff", "\u061c", "\u00ad", "\u2060", "\u2028", "\u2029", "\u0007", "\u007F", "\u3164"];
const hex = (ch: string): string => `U+${(ch.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, "0")}`;
const UNSAFE = /[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}]/u;

const attribution = (over: Partial<Attribution> = {}): Attribution => ({ source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana" }, evidence: {}, ...over });

/** Every character the browser would draw as itself or not at all, in any text node or attribute under `root`. */
function rawUnsafe(root: ParentNode): string[] {
  const found: string[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    if (node.nodeType === Node.TEXT_NODE) {
      for (const ch of node.textContent ?? "") if (ch !== "\t" && UNSAFE.test(ch)) found.push(`text ${hex(ch)}`);
    } else {
      for (const attr of Array.from((node as Element).attributes)) for (const ch of attr.value) if (ch !== "\t" && UNSAFE.test(ch)) found.push(`@${attr.name} ${hex(ch)}`);
    }
  }
  return found;
}

describe("the chip, with text it cannot trust", () => {
  it.each([...BIDI, ...INVISIBLE])("shows %j in an OS login as a marker, never as itself", (ch) => {
    const { container } = render(<AttributionChip attribution={attribution({ actor: { kind: "os_login", login: `da${ch}na` } })} />);
    expect(rawUnsafe(container)).toEqual([]);
    expect(container.textContent).toContain(`<${hex(ch)}>`);
    expect(screen.getByRole("img", { name: new RegExp(hex(ch).replace("+", "\\+")) })).toBeInTheDocument();
  });

  it("shows the classic spoof for what it is", () => {
    const { container } = render(<AttributionChip attribution={attribution({ actor: { kind: "os_login", login: "invoice\u202egnp.exe" } })} />);
    expect(container.textContent).toContain("invoice<U+202E>gnp.exe");
    expect(container.querySelector("bdi")).not.toBeNull();
  });

  it("marks hidden characters in a job name, in a name from the user list and in the evidence", async () => {
    const user = userEvent.setup();
    const job = attribution({ source: "sync_job", actor: { kind: "job", job: { name: "csp\u200b-sit7", uid: "u\u202e1" } }, evidence: { executable: "v\u200di", login_identity: "do\u2066kye" } });
    const { container } = render(<AttributionChip attribution={job} />);
    expect(container.textContent).toContain("Sync job csp<U+200B>-sit7");
    await user.click(screen.getByRole("button"));
    const dialog = await screen.findByRole("dialog", { name: "Attribution evidence" });
    expect(rawUnsafe(dialog)).toEqual([]);
    expect(dialog.textContent).toContain("v<U+200D>i");
    expect(dialog.textContent).toContain("do<U+2066>kye");
    expect(dialog.textContent).toContain("u<U+202E>1");
  });

  it("marks hidden characters in the name the lookup gives for a person", () => {
    const other = "11111111-1111-4111-8111-111111111111:3f2a91bc-0000-4000-8000-000000000003";
    const { container } = render(<AttributionChip attribution={attribution({ source: "tool_write", actor: { kind: "user", user: other } })} nameOf={() => "Marco\u202e Diaz"} />);
    expect(rawUnsafe(container)).toEqual([]);
    expect(container.textContent).toContain("Marco<U+202E> Diaz");
  });

  it.each(["<img src=x onerror=alert(1)>", "<script>alert(1)</script>", "</button><a href=javascript:alert(1)>x</a>", "\"><svg onload=alert(1)>", "{{constructor.constructor('alert(1)')()}}", "javascript:alert(1)"])(
    "keeps %s as text: no element, no handler, no link",
    async (login) => {
      const user = userEvent.setup();
      const { container } = render(<AttributionChip attribution={attribution({ actor: { kind: "os_login", login }, evidence: { executable: login, login_identity: login } })} />);
      expect(container.textContent).toContain(login);
      await user.click(screen.getByRole("button"));
      const dialog = await screen.findByRole("dialog", { name: "Attribution evidence" });
      for (const root of [container, dialog]) {
        expect(root.querySelector("img, script, svg, iframe, object, embed, a[href]")).toBeNull();
        for (const el of root.querySelectorAll("*")) for (const attr of Array.from(el.attributes)) expect(attr.name.startsWith("on"), `${attr.name} on <${el.tagName.toLowerCase()}>`).toBe(false);
      }
    },
  );

  it("keeps a very long name in one line with an ellipsis, and the whole name in the page for assistive technology", () => {
    const login = "a".repeat(256);
    const { container } = render(<AttributionChip attribution={attribution({ actor: { kind: "os_login", login } })} />);
    const name = container.querySelector("bdi");
    expect(name?.className).toContain("truncate");
    expect(name?.textContent).toBe(login);
  });

  it("puts no name in an attribute: a title or a label could carry a bidi override in a place SafeText does not reach", () => {
    const { container } = render(<AttributionChip attribution={attribution({ actor: { kind: "os_login", login: "da\u202ena" } })} />);
    for (const el of container.querySelectorAll("[title], [aria-label]")) {
      expect(el.getAttribute("title") ?? "").not.toContain("da");
      expect(el.getAttribute("aria-label") ?? "").not.toContain("da");
    }
  });
});

describe("the other parts, with text they cannot trust", () => {
  it("draws the evidence list through SafeText, every value", () => {
    const { container } = render(
      <AttributionEvidence attribution={attribution({ actor: { kind: "os_login", login: "d\u202ea" }, evidence: { executable: "e\u200bx", login_identity: "l\ufeffi" } })} />,
    );
    expect(rawUnsafe(container)).toEqual([]);
    const dl = within(container);
    expect(dl.getAllByRole("img").map((m) => m.getAttribute("aria-label"))).toEqual(expect.arrayContaining(["U+202E right-to-left override", "U+200B zero width space", "U+FEFF zero width no-break space"]));
  });

  it("has nothing hostile to draw in a source cell: its words are fixed", () => {
    const { container } = render(<AttributionSourceCell attribution={attribution({ actor: { kind: "os_login", login: "d\u202ea" } })} />);
    expect(rawUnsafe(container)).toEqual([]);
    expect(container.textContent).not.toContain("da");
  });
});
