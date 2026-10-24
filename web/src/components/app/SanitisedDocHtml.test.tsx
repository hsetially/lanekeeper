import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import type { RenderedDoc } from "@/api/generated/model";
import { SanitisedDocHtml } from "./SanitisedDocHtml";

const doc = (html: string): RenderedDoc => ({ path: "guide/intro.md", html, headings: [], content_hash: "a".repeat(64) });

describe("SanitisedDocHtml", () => {
  it("renders the server's HTML", () => {
    const { container } = render(<SanitisedDocHtml doc={doc('<h1 id="doc-intro">Intro</h1><p>Hello <a href="/docs/view?path=a.md" rel="noopener noreferrer">link</a></p>')} />);
    expect(container.querySelector("h1#doc-intro")?.textContent).toBe("Intro");
    expect(container.querySelector("a")?.getAttribute("href")).toBe("/docs/view?path=a.md");
  });

  it("hands links to the browser as the server sent them, never decoded", () => {
    const { container } = render(<SanitisedDocHtml doc={doc('<a href="/%2F%2Fevil.example/" rel="noopener noreferrer">x</a>')} />);
    expect(container.querySelector("a")?.getAttribute("href")).toBe("/%2F%2Fevil.example/");
  });

  it("accepts a RenderedDoc and nothing else", () => {
    // @ts-expect-error a bare string is not a RenderedDoc: the HTML sink cannot be reached with arbitrary text
    const bad = <SanitisedDocHtml doc="<img src=x onerror=alert(1)>" />;
    expect(bad).toBeDefined();
    // @ts-expect-error an object that is not a RenderedDoc is refused too
    const alsoBad = <SanitisedDocHtml doc={{ html: "<p>x</p>" }} />;
    expect(alsoBad).toBeDefined();
  });
});
