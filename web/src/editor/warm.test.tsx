import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Role } from "@/api/generated/model";
import { SessionContext, type SessionState } from "@/auth/session";
import { meBody } from "@/mocks/scenarios";
import { resetWarmForTests, shouldWarm, useEditorWarmup, warmEditor, warmFiles, WARM_META_NAME } from "./warm";

// Edit mode opens the editor, a lazy chunk of a few megabytes. For an Editor and above, once the file view is up, its files
// are FETCHED while the browser is idle, so that the HTTP cache has them when Edit mode asks. They are never imported:
// evaluating Monaco is the expensive part and installs global patches (src/editor/global-patches.ts). A Viewer, a page that
// is not interactive yet, a hidden tab and `Save-Data` all get nothing.

const FILES = ["/assets/entry-ED.js", "/assets/editor-core-C.js", "/assets/entry-ED.css", "/assets/editor.worker-WK.js"];

function withMeta(content: string | null): Document {
  const doc = document.implementation.createHTMLDocument("t");
  if (content !== null) {
    const meta = doc.createElement("meta");
    meta.setAttribute("name", WARM_META_NAME);
    meta.setAttribute("content", content);
    doc.head.append(meta);
  }
  return doc;
}

function bodyOf(chunks: number): Response {
  let sent = 0;
  return new Response(new ReadableStream<Uint8Array>({ pull(controller) { if (sent++ < chunks) controller.enqueue(new Uint8Array(1024)); else controller.close(); } }));
}

beforeEach(() => resetWarmForTests());
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("warmFiles", () => {
  it("reads the files the build named", () => {
    expect(warmFiles(withMeta(FILES.join(" ")))).toEqual(FILES);
  });

  it("is empty when the build wrote none (a build without Edit mode, or the dev server)", () => {
    expect(warmFiles(withMeta(null))).toEqual([]);
    expect(warmFiles(withMeta(""))).toEqual([]);
  });

  it("keeps only files of the app's own /assets/ that are scripts or styles, whatever the page says", () => {
    const hostile = ["https://evil.example/a.js", "//evil.example/a.js", "/assets/../secrets.js", "/other/a.js", "/assets/a.html", "/assets/sub/a.js", "javascript:alert(1)", "assets/a.js", "/assets/ok.js", "/assets/a b.js"];
    expect(warmFiles(withMeta(hostile.join(" ")))).toEqual(["/assets/ok.js"]);
  });

  it("names each file once and at most eight", () => {
    const many = Array.from({ length: 30 }, (_, i) => `/assets/f${i}.js`);
    expect(warmFiles(withMeta([...many, ...many].join(" "))).length).toBe(8);
    expect(warmFiles(withMeta("/assets/a.js /assets/a.js"))).toEqual(["/assets/a.js"]);
  });
});

describe("shouldWarm", () => {
  const base = { role: Role.editor, interactive: true, saveData: false, hidden: false };
  it("is yes for an Editor and above, on an interactive page, in a visible tab, without Save-Data", () => {
    for (const role of [Role.editor, Role.operator, Role.admin]) expect(shouldWarm({ ...base, role })).toBe(true);
  });
  it("is no for a Viewer and for no role", () => {
    expect(shouldWarm({ ...base, role: Role.viewer })).toBe(false);
    expect(shouldWarm({ ...base, role: null })).toBe(false);
  });
  it("is no before the page is interactive, in a hidden tab, and with Save-Data", () => {
    expect(shouldWarm({ ...base, interactive: false })).toBe(false);
    expect(shouldWarm({ ...base, hidden: true })).toBe(false);
    expect(shouldWarm({ ...base, saveData: true })).toBe(false);
  });
});

describe("warmEditor", () => {
  it("fetches each file, one after the other, at low priority, and reads each to the end", async () => {
    const order: string[] = [];
    const fetcher = vi.fn((url: string, _init?: RequestInit) => (order.push(url), Promise.resolve(bodyOf(3))));
    await expect(warmEditor({ doc: withMeta(FILES.join(" ")), fetcher })).resolves.toBe("warmed");
    expect(order).toEqual(FILES);
    for (const call of fetcher.mock.calls) expect(call[1]).toMatchObject({ credentials: "same-origin", priority: "low" });
  });

  it("never imports the editor: nothing of Monaco exists afterwards", async () => {
    await warmEditor({ doc: withMeta(FILES.join(" ")), fetcher: () => Promise.resolve(bodyOf(1)) });
    expect((globalThis as { MonacoEnvironment?: unknown }).MonacoEnvironment).toBeUndefined();
    expect(document.querySelector("script[src*='entry-']")).toBeNull();
    expect(document.querySelector("link[rel='modulepreload']")).toBeNull();
  });

  it("holds none of a body in memory: it reads and drops each piece", async () => {
    let held = 0;
    const stream = new ReadableStream<Uint8Array>({ pull(controller) { held += 1; if (held <= 50) controller.enqueue(new Uint8Array(65_536)); else controller.close(); } });
    const result = await warmEditor({ doc: withMeta("/assets/a.js"), fetcher: () => Promise.resolve(new Response(stream)) });
    expect(result).toBe("warmed");
    expect(held).toBeGreaterThan(50);
  });

  it("does nothing, and says so, when there are no files", async () => {
    const fetcher = vi.fn();
    await expect(warmEditor({ doc: withMeta(null), fetcher })).resolves.toBe("skipped");
    expect(fetcher).not.toHaveBeenCalled();
  });

  it("goes only once a page: a second call finds the files already fetched", async () => {
    const fetcher = vi.fn(() => Promise.resolve(bodyOf(1)));
    await warmEditor({ doc: withMeta("/assets/a.js"), fetcher });
    await expect(warmEditor({ doc: withMeta("/assets/a.js"), fetcher })).resolves.toBe("skipped");
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("fails quietly: a network error or a 404 is not the person's business, and Edit mode will fetch it itself", async () => {
    await expect(warmEditor({ doc: withMeta("/assets/a.js"), fetcher: () => Promise.reject(new TypeError("offline")) })).resolves.toBe("failed");
    resetWarmForTests();
    await expect(warmEditor({ doc: withMeta("/assets/a.js"), fetcher: () => Promise.resolve(new Response("nope", { status: 404 })) })).resolves.toBe("failed");
  });

  it("may be tried again after it failed", async () => {
    await warmEditor({ doc: withMeta("/assets/a.js"), fetcher: () => Promise.reject(new TypeError("offline")) });
    const fetcher = vi.fn(() => Promise.resolve(bodyOf(1)));
    await expect(warmEditor({ doc: withMeta("/assets/a.js"), fetcher })).resolves.toBe("warmed");
  });

  it("stops at once when told to, and fetches no more", async () => {
    const controller = new AbortController();
    const fetcher = vi.fn(() => {
      controller.abort();
      return Promise.resolve(bodyOf(2));
    });
    await expect(warmEditor({ doc: withMeta(FILES.join(" ")), fetcher, signal: controller.signal })).resolves.toBe("skipped");
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("marks when it starts and when it is done, so that a test can tell what it cost", async () => {
    const mark = vi.spyOn(performance, "mark");
    await warmEditor({ doc: withMeta("/assets/a.js"), fetcher: () => Promise.resolve(bodyOf(1)) });
    expect(mark.mock.calls.map((c) => c[0])).toEqual(["lk:editor-warm:start", "lk:editor-warm:done"]);
  });
});

function SessionTestProvider({ role, children }: { role: Role | null; children: React.ReactNode }) {
  const state: SessionState = { status: "ready", me: meBody(role), role };
  return <SessionContext.Provider value={state}>{children}</SessionContext.Provider>;
}

describe("useEditorWarmup", () => {
  function Probe({ interactive }: { interactive: boolean }) {
    useEditorWarmup(interactive, { doc: withMeta("/assets/a.js"), fetcher, schedule: (run) => (idleQueue.push(run), () => undefined) });
    return null;
  }
  const idleQueue: Array<() => void> = [];
  const fetcher = vi.fn((_url: string, _init?: RequestInit) => Promise.resolve(bodyOf(1)));
  beforeEach(() => {
    idleQueue.length = 0;
    fetcher.mockClear();
  });

  const mount = async (role: Role | null, interactive: boolean) => {
    render(
      <SessionTestProvider role={role}>
        <Probe interactive={interactive} />
      </SessionTestProvider>,
    );
    await act(async () => {
      for (const run of idleQueue.splice(0)) run();
      await Promise.resolve();
    });
  };

  it("fetches for an Editor on an interactive page, when idle and not before", async () => {
    render(
      <SessionTestProvider role={Role.editor}>
        <Probe interactive />
      </SessionTestProvider>,
    );
    expect(fetcher).not.toHaveBeenCalled();
    await act(async () => {
      for (const run of idleQueue.splice(0)) run();
      await Promise.resolve();
    });
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("does nothing for a Viewer, for no role, or before the page is interactive", async () => {
    await mount(Role.viewer, true);
    await mount(null, true);
    await mount(Role.admin, false);
    expect(fetcher).not.toHaveBeenCalled();
  });
});
