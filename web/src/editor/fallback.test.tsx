import { act, render, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { resetStaleForTests } from "@/lib/deploy/stale";
import { EditorFallback, useEditorModule } from "./fallback";

// A chunk that cannot be fetched (a deploy removed it, the network dropped) must not leave Edit mode blank. The file is
// shown read-only in the same CodeView the file view uses, with an inline alert that says why and offers another try.

afterEach(() => resetStaleForTests());

const chunkError = () => new TypeError("Failed to fetch dynamically imported module: https://hub.example/assets/entry-ED.js");
const fakeMonaco = { editor: {} } as never;

describe("useEditorModule", () => {
  it("is loading, then ready with the module", async () => {
    const { result } = renderHook(() => useEditorModule(() => Promise.resolve(fakeMonaco)));
    expect(result.current.state).toBe("loading");
    await waitFor(() => expect(result.current.state).toBe("ready"));
    expect(result.current.state === "ready" && result.current.monaco).toBe(fakeMonaco);
  });

  it("is failed when the chunk cannot be fetched, and tries again on request", async () => {
    const load = vi.fn().mockRejectedValueOnce(chunkError()).mockResolvedValueOnce(fakeMonaco);
    const { result } = renderHook(() => useEditorModule(load));
    await waitFor(() => expect(result.current.state).toBe("failed"));
    act(() => {
      if (result.current.state === "failed") result.current.retry();
    });
    await waitFor(() => expect(result.current.state).toBe("ready"));
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("does not set state after it was unmounted", () => {
    let resolve: (m: never) => void = () => undefined;
    const load = () => new Promise<never>((r) => (resolve = r));
    const { unmount } = renderHook(() => useEditorModule(load));
    unmount();
    act(() => {
      resolve(fakeMonaco);
    });
  });
});

describe("EditorFallback", () => {
  it("shows the file read-only, with an alert that says the editor did not load", () => {
    render(<EditorFallback label="a.yml" text={"port: 8080\nname: demo\n"} onRetry={() => undefined} />);
    expect(screen.getByRole("alert")).toHaveTextContent("The editor could not be loaded");
    expect(screen.getByRole("table", { name: "a.yml" })).toBeInTheDocument();
    expect(screen.getByText("port: 8080")).toBeInTheDocument();
  });

  it("says it is read-only, so no one types into it expecting it to save", () => {
    render(<EditorFallback label="a.yml" text="x" onRetry={() => undefined} />);
    expect(screen.getByRole("alert")).toHaveTextContent(/read-only/i);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("offers another try", async () => {
    const user = userEvent.setup();
    const onRetry = vi.fn();
    render(<EditorFallback label="a.yml" text="x" onRetry={onRetry} />);
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});
