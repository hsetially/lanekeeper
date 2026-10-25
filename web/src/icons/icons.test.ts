// Icons are inline lucide-react components (S12: no CSS mask requests; the handoff itself recommends it).
// Every icon file of the handoff must have an export, and the module offers nothing the design lacks.
import { readdirSync } from "node:fs";
import { join } from "node:path";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { describe, expect, it } from "vitest";
import { HANDOFF_DIR } from "../../scripts/handoff/dc-parse.mjs";
import * as icons from "./index";

// Independent of the generator: lucide names are PascalCase of the kebab-case file name.
const pascal = (kebab: string): string =>
  kebab
    .split("-")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join("");

const files = readdirSync(join(HANDOFF_DIR, "assets", "icons"))
  .filter((f) => f.endsWith(".svg"))
  .map((f) => f.replace(/\.svg$/, ""))
  .sort();

describe("icons", () => {
  it("covers the 68 icons of the handoff", () => {
    expect(files).toHaveLength(68);
  });

  it.each(files)("exports %s as a component", (file) => {
    const name = pascal(file);
    const component = (icons as Record<string, unknown>)[name];
    expect(component, `${name} is not exported from src/icons`).toBeDefined();
    const markup = renderToStaticMarkup(createElement(component as React.ComponentType));
    expect(markup).toMatch(/^<svg /);
    expect(markup).not.toContain("style=");
  });

  it("exports no icon the handoff does not have", () => {
    const expected = files.map(pascal).sort();
    expect(Object.keys(icons).sort()).toEqual(expected);
  });

  it("hides the icon from assistive technology unless it is labelled", () => {
    const markup = renderToStaticMarkup(createElement(icons.Check));
    expect(markup).toContain('aria-hidden="true"');
  });
});
