// What a data table (`role=grid` with column headers: the shared DataTable) must do in a real browser, read with boxes:
//   - every cell is under its header (same left edge and width), at rest and after the container is scrolled sideways;
//   - the header stays at the top of its container when the rows scroll under it, and moves sideways with the rows;
//   - the columns add up to what the container can show or the container scrolls (the page never does: audit.ts says that).
// This is the check that the `minWidth` change of the DataTable (S2) was waiting for: unit tests have no layout.
import type { Page } from "@playwright/test";
import type { Defect } from "./audit.ts";

export type TableReport = { tables: number; sideways: number; scrolledDown: number; defects: Defect[] };

export async function auditTables(page: Page): Promise<TableReport> {
  return page.evaluate(async () => {
    const out: TableReport = { tables: 0, sideways: 0, scrolledDown: 0, defects: [] };
    const frame = (): Promise<void> => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    const label = (el: Element): string => `${el.tagName.toLowerCase()}[role=grid]"${el.getAttribute("aria-label") ?? ""}"`;
    const cells = (row: Element, role: string): Map<string, DOMRect> => {
      const m = new Map<string, DOMRect>();
      for (const c of row.querySelectorAll(`[role='${role}'][aria-colindex]`)) m.set(c.getAttribute("aria-colindex") ?? "", c.getBoundingClientRect());
      return m;
    };
    for (const grid of document.querySelectorAll("[role='grid']")) {
      if (getComputedStyle(grid).display === "none" || grid.getClientRects().length === 0) continue;
      const header = grid.querySelector("[role='row'][aria-rowindex='1']");
      if (header === null || header.querySelector("[role='columnheader']") === null) continue;
      const rows = [...grid.querySelectorAll("[role='row']")].filter((r) => r !== header && r.querySelector("[role='gridcell'][aria-colindex]") !== null);
      if (rows.length === 0) continue;
      out.tables += 1;
      const first = rows[0] as Element;

      const aligned = (when: string): void => {
        const h = cells(header, "columnheader");
        const c = cells(first, "gridcell");
        for (const [index, hr] of h) {
          const cr = c.get(index);
          if (cr === undefined) continue;
          if (Math.abs(hr.left - cr.left) > 1.5 || Math.abs(hr.width - cr.width) > 1.5) {
            out.defects.push({ what: `column ${index} is not under its header (${when})`, where: label(grid), detail: `header ${Math.round(hr.left)}+${Math.round(hr.width)}, cell ${Math.round(cr.left)}+${Math.round(cr.width)}` });
            return;
          }
        }
      };
      aligned("at rest");

      // No cell is taller than its row: a row has a fixed height (the rows are virtualised), and what is taller than it runs under the next row.
      let tall = false;
      for (const row of rows.slice(0, 60)) {
        const rowBox = row.getBoundingClientRect();
        for (const cell of row.querySelectorAll("[role='gridcell'][aria-colindex]")) {
          const box = cell.getBoundingClientRect();
          if (box.top < rowBox.top - 1 || box.bottom > rowBox.bottom + 1) {
            tall = true;
            out.defects.push({ what: `a cell is taller than its row (column ${cell.getAttribute("aria-colindex") ?? "?"})`, where: label(grid), detail: `row ${Math.round(rowBox.height)}px, cell ${Math.round(box.height)}px: "${(cell.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 40)}"` });
            break;
          }
        }
        if (tall) break;
      }

      if (grid.scrollWidth > grid.clientWidth + 1) {
        out.sideways += 1;
        const before = header.getBoundingClientRect().left;
        grid.scrollLeft = Math.min(240, grid.scrollWidth - grid.clientWidth);
        await frame();
        aligned("scrolled sideways");
        const moved = before - header.getBoundingClientRect().left;
        const rowMoved = (rows[0] as Element).getBoundingClientRect().left - (rows[0] as Element).getBoundingClientRect().left;
        if (grid.scrollLeft > 0 && moved === 0 && rowMoved === 0) {
          // The header and the rows do not move with the scroll: either both are pinned (wrong) or the container is not the scroller.
          out.defects.push({ what: "the header did not scroll sideways with the rows", where: label(grid), detail: `scrollLeft ${grid.scrollLeft}` });
        }
        grid.scrollLeft = 0;
        await frame();
      }

      if (grid.scrollHeight > grid.clientHeight + 40) {
        out.scrolledDown += 1;
        const top = grid.getBoundingClientRect().top + grid.clientTop;
        const at = header.getBoundingClientRect().top;
        grid.scrollTop = Math.min(160, grid.scrollHeight - grid.clientHeight);
        await frame();
        const now = header.getBoundingClientRect().top;
        if (Math.abs(now - at) > 1.5 || Math.abs(now - top) > 1.5) out.defects.push({ what: "the header left the top of its container when the rows scrolled", where: label(grid), detail: `header top ${Math.round(at)} then ${Math.round(now)}, container top ${Math.round(top)}` });
        grid.scrollTop = 0;
        await frame();
      }
    }
    return out;
  });
}
