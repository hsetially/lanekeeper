import { describe, expect, it } from "vitest";
import { isContentWithheld, withheldReason } from "./withheld";

// Whether the server withholds a file's content, and why. The card the page draws for such a file is `WithheldCard` in
// features/file/view/ContentView.tsx, and it is tested there (ContentView.test.tsx) and through the page (features/masking/withheld.dom.test.tsx).

describe("isContentWithheld", () => {
  it("is true when the server says the content is withheld, or the file is of the denied class, either alone", () => {
    expect(isContentWithheld({ content_withheld: true })).toBe(true);
    expect(isContentWithheld({ class: "denied" })).toBe(true);
    expect(isContentWithheld({ content_withheld: false, class: "denied" })).toBe(true);
    expect(isContentWithheld({ content_withheld: true, class: "text" })).toBe(true);
  });

  it("is false for a file whose content is there, and for nothing at all (nothing is known, so nothing is claimed)", () => {
    expect(isContentWithheld({ content_withheld: false, class: "structured" })).toBe(false);
    expect(isContentWithheld({ class: "binary" })).toBe(false);
    expect(isContentWithheld({})).toBe(false);
    expect(isContentWithheld(null)).toBe(false);
    expect(isContentWithheld(undefined)).toBe(false);
  });
});

describe("withheldReason", () => {
  it("is denied for the denied class, unchecked for any other file the server withholds, and null otherwise", () => {
    expect(withheldReason({ class: "denied", content_withheld: true })).toBe("denied");
    expect(withheldReason({ class: "denied" })).toBe("denied");
    expect(withheldReason({ class: "structured", content_withheld: true })).toBe("unchecked");
    expect(withheldReason({ content_withheld: true })).toBe("unchecked");
    expect(withheldReason({ class: "structured", content_withheld: false })).toBeNull();
    expect(withheldReason(null)).toBeNull();
  });
});
