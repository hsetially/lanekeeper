import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { binaryServedFile, maskedServedFile, maskOf, MARKER, servedFile, servedText, withheldServedFile } from "@/mocks/design/served/data";
import { expectNoAxeViolations } from "@/test/axe";
import { ServedText } from "./ServedText";

// The text of a served file (S14b, M1): the config-server's answer, drawn as it came, read-only. The web masks nothing; where the hub
// masked, the text holds a marker and `masks` says where, and it is drawn as a "Hidden value" token with no control at all (the
// served masks name no setting, so there is no reveal). A file the hub withholds shows no text, whatever else the answer holds.

const SECRET = "hunter2-correct-horse-battery";
const cells = (row: HTMLElement) => within(row).getAllByRole("cell");
const view = (served = servedFile()) => render(<ServedText served={served} label="Served text of connection-pool.yml" />);

describe("a text without masks", () => {
  it("shows the lines with their numbers, as the code view of a file does", () => {
    view();
    const table = screen.getByRole("table", { name: "Served text of connection-pool.yml" });
    const rows = within(table).getAllByRole("row");
    expect(rows).toHaveLength(8);
    expect(cells(rows[0] as HTMLElement).map((c) => c.textContent)).toEqual(["1", "# connection-pool.yml, as served to tx-infinity-core for sit7"]);
    expect(cells(rows[7] as HTMLElement).map((c) => c.textContent)).toEqual(["8", "  audit: ${AUDIT_SINK}"]);
  });

  it("says how many lines, and that it is read-only", () => {
    view();
    expect(screen.getByText("8 lines")).toBeInTheDocument();
    expect(screen.getByText(/read-only/i)).toBeInTheDocument();
  });

  it("offers nothing to press: no copy, no reveal, no edit", () => {
    view();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("draws hidden and bidirectional characters as visible markers, never as themselves", () => {
    view(servedFile({ text: "name: invoice‮gnp.exe\nzero: a​b\n" }));
    expect(screen.getAllByRole("img", { name: /U\+202E/i })).toHaveLength(1);
    expect(screen.getAllByRole("img", { name: /U\+200B/i })).toHaveLength(1);
    expect(document.body.textContent).not.toContain("‮");
  });

  it("draws markup as text and never as markup", () => {
    view(servedFile({ text: '<img src=x onerror="alert(1)">\n<script>alert(2)</script>\n' }));
    expect(document.querySelector("img[src='x']")).toBeNull();
    expect(document.querySelector("script")).toBeNull();
    expect(screen.getByText('<img src=x onerror="alert(1)">')).toBeInTheDocument();
  });

  it("keeps a CRLF file as lines, with no carriage return drawn as a marker", () => {
    view(servedFile({ text: "a: 1\r\nb: 2\r\n" }));
    expect(screen.getAllByRole("row")).toHaveLength(2);
    expect(screen.queryByText("<U+000D>")).toBeNull();
    expect(screen.queryAllByRole("img")).toHaveLength(0);
  });

  it("says so for an empty file, and does not call it a failure", () => {
    view(servedFile({ text: "" }));
    expect(screen.getByText(/served an empty file/i)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });
});

describe("a text the hub masked", () => {
  it("draws each marker as a hidden value, with its length, and not as the marker's words", () => {
    view(maskedServedFile());
    const row = screen.getAllByRole("row")[6] as HTMLElement;
    expect(row.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    expect(row).toHaveTextContent("Hidden value");
    expect(row).toHaveTextContent("22 chars");
    expect(row.textContent).not.toContain("REDACTED");
    expect(row.textContent).toContain("password:");
  });

  it("has no Reveal control, even when a mask names a setting: a served text is read-only and there is no reveal from it", () => {
    const text = servedText();
    view(servedFile({ text, masks: [{ ...maskOf(text, 7, MARKER), setting: "pool.password" }] }));
    expect(document.querySelector("[data-slot='hidden-value']")).not.toBeNull();
    expect(screen.queryByRole("button", { name: /reveal/i })).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("recognises a marker the list does not name by its form, and draws it as a hidden value too", () => {
    const text = `a: ${MARKER}\nb: [REDACTED: 5 chars, jwt]\n`;
    view(servedFile({ text, masks: [maskOf(text, 1, MARKER)] }));
    expect(document.querySelectorAll("[data-slot='hidden-value']")).toHaveLength(2);
  });

  it("works with multi-byte characters before the marker", () => {
    const text = `é😀: 1\n名前: ${MARKER}\n`;
    view(servedFile({ text, masks: [maskOf(text, 2, MARKER)] }));
    expect(screen.getAllByRole("row")[1]?.querySelector("[data-slot='hidden-value']")).not.toBeNull();
  });

  it("holds no value anywhere: the planted secret is in no text, attribute or name", () => {
    view(maskedServedFile());
    expect(document.documentElement.outerHTML).not.toContain(SECRET);
  });
});

describe("a mask that cannot be trusted fails closed", () => {
  it("hides the whole line of a mask whose span is not a marker, so a value under it is never drawn", () => {
    const text = `a: 1\nsecret: ${SECRET}\nb: 2\n`;
    // The server says a marker sits where the value is: it does not, so nothing of the line may be drawn.
    const start = text.indexOf(SECRET);
    view(servedFile({ text, masks: [{ line: 2, start, end: start + SECRET.length }] }));
    expect(document.documentElement.outerHTML).not.toContain(SECRET);
    expect(screen.getByText("Hidden line")).toBeInTheDocument();
    expect(screen.getByText("a: 1")).toBeInTheDocument();
    expect(screen.getByText("b: 2")).toBeInTheDocument();
  });

  it("refuses the whole text when a mask names a line the text does not have", () => {
    const text = `a: 1\nsecret: ${SECRET}\n`;
    view(servedFile({ text, masks: [{ line: 99, start: 0, end: 5 }] }));
    expect(screen.getByText(/can't be shown safely/i)).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    expect(document.documentElement.outerHTML).not.toContain(SECRET);
  });

  it("refuses the whole text when there are more masks than a text can have", () => {
    const text = `a: ${MARKER}\n`;
    const one = maskOf(text, 1, MARKER);
    view(servedFile({ text: `${SECRET}\n${text}`, masks: Array.from({ length: 1_001 }, () => one) }));
    expect(screen.getByText(/can't be shown safely/i)).toBeInTheDocument();
    expect(document.documentElement.outerHTML).not.toContain(SECRET);
  });
});

describe("a file the hub withholds", () => {
  it("shows no text and says why there is none", () => {
    view(withheldServedFile());
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.getByText("Content withheld")).toBeInTheDocument();
    expect(screen.getByText(/never shows|could not be checked/i)).toBeInTheDocument();
  });

  it("shows no text even if the answer holds some: withheld wins over a text that should not be there", () => {
    view(withheldServedFile({ text: `key: ${SECRET}\n` }));
    expect(screen.queryByRole("table")).toBeNull();
    expect(document.documentElement.outerHTML).not.toContain(SECRET);
    expect(screen.getByText("Content withheld")).toBeInTheDocument();
  });

  it("offers nothing to press", () => {
    view(withheldServedFile());
    expect(screen.queryAllByRole("button")).toHaveLength(0);
    expect(screen.queryAllByRole("link")).toHaveLength(0);
  });
});

describe("an answer that is not text", () => {
  it("says it is not shown as text, and is not a failure or a withheld file", () => {
    view(binaryServedFile());
    expect(screen.getByText("Not shown as text")).toBeInTheDocument();
    expect(screen.queryByText("Content withheld")).toBeNull();
    expect(screen.queryByRole("table")).toBeNull();
  });
});

describe("accessibility", () => {
  it.each([
    ["a text", servedFile()],
    ["a masked text", maskedServedFile()],
    ["a withheld file", withheldServedFile()],
    ["a file that is not text", binaryServedFile()],
  ])("has no structural violation for %s", async (_name, served) => {
    view(served);
    await expectNoAxeViolations();
  });
});
