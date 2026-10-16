import { describe, expect, it } from "vitest";
import { buildBoards, parseBoard, parseImportAttrs, renderBoardsTs } from "./extract-boards.mjs";

describe("parseImportAttrs", () => {
  it("types the {{ }} literals and flags unresolved bindings as dynamic", () => {
    const a = parseImportAttrs(`name="Lanekeeper Shell" theme="dark" toast="{{ true }}" job-step="{{ 2 }}" role="{{ role }}" hint-size="1440px,900px" style="flex:1"`);
    expect(a.name).toBe("Lanekeeper Shell");
    expect(a.theme).toBe("dark");
    expect(a.size).toEqual({ w: 1440, h: 900 });
    expect(a.props).toEqual({ toast: true, "job-step": 2, role: "{{ role }}" });
    expect(a.dynamic).toBe(true);
  });

  it("is not dynamic when every value is a literal", () => {
    const a = parseImportAttrs(`name="X" page="swimlanes" cdrawer="{{ false }}" hint-size="100%,100%"`);
    expect(a.dynamic).toBe(false);
    expect(a.props).toEqual({ page: "swimlanes", cdrawer: false });
    expect(a.size).toEqual({ w: "100%", h: "100%" });
  });
});

describe("parseBoard", () => {
  const html = `
<section class="dv-turn" id="t1"><span class="dv-tname">Swimlanes overview</span>
<div class="dv-grp">Six swimlanes</div>
<div class="dv-opt" id="1a"><div class="dv-olabel"><a class="dv-oid" href="#1a">1a</a>Light &amp; Admin</div><div class="dv-card"><dc-import name="Lanekeeper Shell" theme="light" page="swimlanes" hint-size="1440px,900px"></dc-import></div></div>
<div class="dv-opt" id="1b"><div class="dv-olabel"><a class="dv-oid" href="#1b">1b</a>Specimen only</div><div class="dv-card"><div>inline</div></div></div>
</section>`;

  it("attaches each import to its labelled option, turn and group", () => {
    const { frames, optionsWithoutImport } = parseBoard("Board.dc.html", html);
    expect(frames).toHaveLength(1);
    expect(frames[0]).toMatchObject({
      board: "Board.dc.html", turn: "t1", turnName: "Swimlanes overview", group: "Six swimlanes", id: "1a", label: "Light & Admin",
      labelled: true, page: "Lanekeeper Shell", theme: "light", props: { page: "swimlanes" },
    });
    expect(optionsWithoutImport).toEqual(["1b"]);
  });
});

describe("extract-boards on the real handoff", () => {
  const data = buildBoards();

  it("finds all 293 imports: 283 labelled frames and the Shell's 10 embedded pages", () => {
    expect(data.frames.length + data.embedded.length).toBe(293);
    expect(data.frames).toHaveLength(283);
    expect(data.embedded).toHaveLength(10);
    expect(data.embedded.every((e) => e.board === "Lanekeeper Shell.dc.html" && e.dynamic)).toBe(true);
    expect(data.frames.every((f) => f.labelled && f.id && f.label)).toBe(true);
  });

  it("counts the frames per board", () => {
    const per = Object.fromEntries(Object.entries(Object.groupBy(data.frames, (f) => f.board)).map(([k, v]) => [k, v.length]));
    expect(per).toEqual({
      "Lanekeeper Access and Onboarding.dc.html": 20,
      "Lanekeeper Admin.dc.html": 66,
      "Lanekeeper App Shell.dc.html": 16,
      "Lanekeeper Approvals and Audit.dc.html": 42,
      "Lanekeeper Compare.dc.html": 20,
      "Lanekeeper Consequence Dialogs.dc.html": 17,
      "Lanekeeper File View.dc.html": 15,
      "Lanekeeper Findings and Adoption.dc.html": 39,
      "Lanekeeper Search and Status.dc.html": 24,
      "Lanekeeper Swimlane Detail.dc.html": 14,
      "Lanekeeper Swimlanes.dc.html": 10,
    });
  });

  it("has a unique key per frame and a size on every frame", () => {
    const keys = data.frames.map((f) => `${f.board}#${f.turn}#${f.id}`);
    expect(new Set(keys).size).toBe(keys.length);
    expect(data.frames.every((f) => f.size.w && f.size.h)).toBe(true);
  });

  it("records the declared props and preview size of every imported page", () => {
    expect(Object.keys(data.pageProps)).toContain("Lanekeeper Shell");
    expect(data.pageProps["Lanekeeper Shell"].props.theme).toMatchObject({ default: "light" });
    expect(data.pageProps["Lanekeeper Shell"].preview).toBeDefined();
  });

  it("renders a typed module", () => {
    expect(renderBoardsTs(data)).toContain("export const FRAMES = ");
  });
});
