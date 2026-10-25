import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The environment must be in place before Monaco evaluates a module (F7): this is the unit proof of what it does. The
// editor itself cannot run in jsdom; e2e/editor/csp.spec.ts runs the real thing under the production header.
vi.mock("monaco-editor/editor/editor.worker.js?worker&url", () => ({ default: "/assets/editor.worker-TEST.js" }));

type Created = { name: string; rules: { createHTML?: (s: string) => string; createScriptURL?: (s: string) => string } };

describe("installEditorEnvironment", () => {
  let created: Created[];
  let workers: Array<{ url: string; options: WorkerOptions | undefined }>;

  beforeEach(async () => {
    vi.resetModules();
    created = [];
    workers = [];
    vi.stubGlobal("window", {
      location: { origin: "https://hub.example" },
      trustedTypes: {
        createPolicy: (name: string, rules: Created["rules"]) => {
          created.push({ name, rules });
          return { name, createHTML: (s: string) => ({ toString: () => rules.createHTML?.(s) ?? s }), createScriptURL: (s: string) => ({ toString: () => rules.createScriptURL?.(s) ?? s }) };
        },
      },
    });
    vi.stubGlobal("location", { origin: "https://hub.example" });
    vi.stubGlobal(
      "Worker",
      vi.fn(function (this: unknown, url: unknown, options?: WorkerOptions) {
        workers.push({ url: String(url), options });
      }),
    );
    const trusted = await import("@/lib/csp/trusted-types");
    trusted.resetPoliciesForTests();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    Reflect.deleteProperty(globalThis, "MonacoEnvironment");
  });

  it("creates exactly one policy, lanekeeper-monaco, and nothing else, before any Monaco module exists", async () => {
    const { installEditorEnvironment } = await import("./environment");
    installEditorEnvironment();
    expect(created.map((c) => c.name)).toEqual(["lanekeeper-monaco"]);
  });

  it("answers every policy name Monaco asks for with that one policy, without creating another", async () => {
    const { installEditorEnvironment } = await import("./environment");
    const { askedNames } = installEditorEnvironment();
    const env = globalThis.MonacoEnvironment;
    const names = ["defaultWorkerFactory", "diffEditorWidget", "diffReview", "domLineBreaksComputer", "editorGhostText", "editorViewLayer", "richScreenReaderContent", "standaloneColorizer", "stickyScrollViewLayer", "tokenizeToString", "domSanitize3"];
    const policies = names.map((n) => env?.createTrustedTypesPolicy?.(n));
    expect(new Set(policies).size).toBe(1);
    expect(policies[0]?.name).toBe("lanekeeper-monaco");
    expect(askedNames).toEqual(names);
    expect(created).toHaveLength(1);
  });

  it("is idempotent: a second call neither creates a policy nor installs the patches again", async () => {
    const { installEditorEnvironment } = await import("./environment");
    const first = installEditorEnvironment();
    const second = installEditorEnvironment();
    expect(second).toBe(first);
    expect(created).toHaveLength(1);
  });

  it("rewrites style attributes in the HTML the policy makes, and leaves text alone", async () => {
    const { installEditorEnvironment } = await import("./environment");
    installEditorEnvironment();
    const html = created[0]?.rules.createHTML?.('<div style="top:0px">style="x"</div>') ?? "";
    expect(html).not.toMatch(/<div style=/);
    expect(html).toMatch(/<div data-lk-s-[0-9a-f]{16}="top:0px">style="x"<\/div>/);
  });

  it("makes the worker from the built same-origin script, through the policy, and refuses any other URL", async () => {
    const { installEditorEnvironment } = await import("./environment");
    installEditorEnvironment();
    void globalThis.MonacoEnvironment?.getWorker?.("worker", "yaml");
    expect(workers).toEqual([{ url: "https://hub.example/assets/editor.worker-TEST.js", options: { name: "yaml" } }]);
    expect(() => created[0]?.rules.createScriptURL?.("https://evil.example/assets/x.js")).toThrow(TypeError);
    expect(() => created[0]?.rules.createScriptURL?.("/assets/x.js?x=1")).toThrow(TypeError);
  });
});
