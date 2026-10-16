import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Banner } from "./Banner";
import { CopyButton } from "./CopyButton";
import { RequestId } from "./RequestId";
import { TruncationBanner } from "./TruncationBanner";
import { WriteError } from "./WriteError";

describe("Banner", () => {
  it("shows title, description and an action, as a status", () => {
    render(
      <Banner variant="warning" title="NFS ahead" action={<button type="button">Raise PR</button>}>
        The file differs from Git.
      </Banner>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("NFS ahead");
    expect(screen.getByRole("status")).toHaveTextContent("The file differs from Git.");
    expect(screen.getByRole("button", { name: "Raise PR" })).toBeInTheDocument();
  });

  it("danger is an alert", () => {
    render(<Banner variant="danger" title="Offline" />);
    expect(screen.getByRole("alert")).toHaveTextContent("Offline");
  });

  it("is dismissible only when it is a success", async () => {
    const user = userEvent.setup();
    let dismissed = 0;
    const { rerender } = render(<Banner variant="success" title="Saved" onDismiss={() => (dismissed += 1)} />);
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(dismissed).toBe(1);
    for (const variant of ["info", "warning", "danger"] as const) {
      rerender(<Banner variant={variant} title="x" onDismiss={() => (dismissed += 1)} />);
      expect(screen.queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();
    }
  });
});

describe("TruncationBanner", () => {
  it("says that the result was cut off, how many are shown, and what to do, never silently", () => {
    render(<TruncationBanner shown={500} noun="matches" />);
    const banner = screen.getByRole("status");
    expect(banner).toHaveTextContent("500");
    expect(banner).toHaveTextContent("matches");
    expect(banner).toHaveTextContent(/narrow/i);
  });
});

describe("WriteError", () => {
  it("states what happened and gives a copyable request ID", () => {
    render(<WriteError title="Could not save" detail="The agent did not answer in time." requestId="req_01HXYZ" />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Could not save");
    expect(alert).toHaveTextContent("The agent did not answer in time.");
    expect(alert).toHaveTextContent("req_01HXYZ");
    expect(screen.getByRole("button", { name: "Copy request ID" })).toBeInTheDocument();
  });
});

describe("CopyButton and RequestId", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("copies the exact value, switches to Copied, announces it politely and goes back after 1.5 s", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    render(<CopyButton value="a3f9c21" what="hash" />);
    const button = screen.getByRole("button", { name: "Copy hash" });
    await user.click(button);
    expect(await navigator.clipboard.readText()).toBe("a3f9c21");
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(screen.getByText("Copied", { selector: '[aria-live="polite"]' })).toBeInTheDocument();
    await act(async () => {
      vi.advanceTimersByTime(1500);
    });
    expect(screen.getByRole("button", { name: "Copy hash" })).toBeInTheDocument();
    expect(screen.queryByText("Copied", { selector: '[aria-live="polite"]' })).not.toBeInTheDocument();
  });

  it("reports a failed copy instead of throwing", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValueOnce(new Error("denied"));
    render(<CopyButton value="x" what="path" />);
    await user.click(screen.getByRole("button", { name: "Copy path" }));
    expect(await screen.findByText("Copy failed", { selector: '[aria-live="polite"]' })).toBeInTheDocument();
  });

  it("RequestId shows the id in mono next to a copy button", async () => {
    const user = userEvent.setup();
    render(<RequestId id="req_01HXYZ" />);
    expect(screen.getByText("Request ID")).toBeInTheDocument();
    expect(screen.getByText("req_01HXYZ")).toHaveClass("font-mono");
    await user.click(screen.getByRole("button", { name: "Copy request ID" }));
    expect(await navigator.clipboard.readText()).toBe("req_01HXYZ");
  });
});
