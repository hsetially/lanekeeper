import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ApiProblem } from "@/api/problem";
import { Role } from "@/api/generated/model";
import { Layers } from "@/icons";
import { EmptyState } from "./EmptyState";
import { ErrorState } from "./ErrorState";
import { PageSkeleton } from "./PageSkeleton";
import { PermissionState } from "./PermissionState";

// The three states every screen shares (the States index: Empty, Error, Permission) and the loading skeleton of a route. They
// say what happened in the design's words, show the request id for an error, and show nothing a server wrote unless the screen
// named the code as safe (S21).

const problem = (init: Partial<ConstructorParameters<typeof ApiProblem>[0]> = {}) =>
  new ApiProblem({ status: 500, kind: "internal", title: "Internal error", requestId: "req-abc-1", detail: "stack: secret-ish internals", ...init });

describe("EmptyState", () => {
  it("says what is missing, why, and offers the next step", () => {
    render(
      <EmptyState icon={Layers} title="No swimlanes registered yet" action={<button>Add a swimlane</button>}>
        A swimlane appears here once an agent registers for its Kubernetes namespace.
      </EmptyState>,
    );
    expect(screen.getByText("No swimlanes registered yet")).toBeInTheDocument();
    expect(screen.getByText(/once an agent registers/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a swimlane" })).toBeInTheDocument();
  });

  it("hides its icon from assistive technology and works with a title alone", () => {
    const { container } = render(<EmptyState title="Nothing here" />);
    expect(container.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    expect(container.querySelectorAll("p")).toHaveLength(1);
  });

  it("makes its title a heading when the screen asks, at the level it asks", () => {
    render(<EmptyState title="Nothing here" as="h1" />);
    expect(screen.getByRole("heading", { level: 1, name: "Nothing here" })).toBeInTheDocument();
  });
});

describe("ErrorState", () => {
  it("shows the title, the problem's title and the request id with a copy button, as an alert", () => {
    render(<ErrorState title="Couldn't load swimlanes" problem={problem()}>The server returned an error while reading the swimlane list.</ErrorState>);
    const alert = screen.getByRole("alert");
    expect(within(alert).getByText("Couldn't load swimlanes")).toBeInTheDocument();
    expect(within(alert).getByText("Internal error")).toBeInTheDocument();
    expect(within(alert).getByText("req-abc-1")).toBeInTheDocument();
    expect(within(alert).getByRole("button", { name: /copy request id/i })).toBeInTheDocument();
  });

  it("offers the details for support next to the request id: the id, the problem, its type, the status and the time, and nothing else", async () => {
    const user = userEvent.setup();
    render(<ErrorState title="Couldn't load swimlanes" problem={problem({ status: 503, kind: "unavailable", title: "The hub is busy" })} />);
    const alert = screen.getByRole("alert");
    await user.click(within(alert).getByRole("button", { name: "Copy details for support" }));
    const copied = await navigator.clipboard.readText();
    expect(copied.split("\n")).toEqual([
      "Request ID: req-abc-1",
      "Problem: The hub is busy",
      "Type: urn:lanekeeper:problem:unavailable",
      "Status: 503",
      expect.stringMatching(/^Time: \d{4}-\d{2}-\d{2} \d{2}:\d{2} UTC$/),
    ]);
    // What the server wrote in `detail` is not in it, whether or not the screen shows it.
    expect(copied).not.toContain("secret-ish");
  });

  it("offers no details when there is no real response to describe (a request id from the address alone, or no request id)", () => {
    const { rerender } = render(<ErrorState title="t" problem={problem({ status: 0, kind: "unknown" })} />);
    expect(screen.queryByRole("button", { name: "Copy details for support" })).toBeNull();
    rerender(<ErrorState title="t" problem={problem({ requestId: undefined })} />);
    expect(screen.queryByRole("button", { name: "Copy details for support" })).toBeNull();
    expect(screen.queryByRole("button", { name: /copy request id/i })).toBeNull();
  });

  it("never shows the server's detail by default, nor the message of an error that is not a problem", () => {
    render(<ErrorState title="Something went wrong" problem={problem()} />);
    expect(screen.queryByText(/secret-ish/)).toBeNull();
  });

  it("shows the detail only for a code the screen named as safe", () => {
    const { rerender } = render(<ErrorState title="t" problem={problem({ kind: "invalid_content", detail: "line 3: tab character" })} detailKinds={["invalid_content"]} />);
    expect(screen.getByText("line 3: tab character")).toBeInTheDocument();
    rerender(<ErrorState title="t" problem={problem({ kind: "internal", detail: "line 3: tab character" })} detailKinds={["invalid_content"]} />);
    expect(screen.queryByText("line 3: tab character")).toBeNull();
  });

  it("offers Retry only when told it can, and calls it", async () => {
    const onRetry = vi.fn();
    const { rerender } = render(<ErrorState title="t" problem={problem()} />);
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    rerender(<ErrorState title="t" problem={problem()} onRetry={onRetry} />);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("works with no problem at all (an error the router caught), and shows no request id then", () => {
    render(<ErrorState title="The page could not be shown" />);
    expect(screen.getByRole("alert")).toHaveTextContent("The page could not be shown");
    expect(screen.queryByText(/Request ID/)).toBeNull();
  });

  it("makes its title a heading at the level the screen asks", () => {
    render(<ErrorState title="Something went wrong" as="h1" />);
    expect(screen.getByRole("heading", { level: 1, name: "Something went wrong" })).toBeInTheDocument();
  });
});

describe("PermissionState", () => {
  it("names the screen and the role it needs, and the role the person has", () => {
    render(<PermissionState what="Approvals" need={Role.admin} have={Role.editor} />);
    expect(screen.getByText("Approvals needs the Admin role")).toBeInTheDocument();
    expect(screen.getByText(/You are signed in as Editor/)).toBeInTheDocument();
    expect(screen.getByText(/Ask an Admin/)).toBeInTheDocument();
  });

  it("says so when the person has no role yet", () => {
    render(<PermissionState what="Swimlanes" need={null} have={null} />);
    expect(screen.getByText("You do not have access")).toBeInTheDocument();
    expect(screen.getByText(/no role yet/i)).toBeInTheDocument();
  });

  it("takes an action, the Viewer's Request a role change, and a heading level", () => {
    render(<PermissionState what="Admin" need={Role.admin} have={Role.viewer} as="h1" action={<button>Request a role change</button>} />);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Admin needs the Admin role");
    expect(screen.getByRole("button", { name: "Request a role change" })).toBeInTheDocument();
  });

  it("has an icon and words, never colour alone", () => {
    const { container } = render(<PermissionState what="Audit" need={Role.admin} have={Role.viewer} />);
    expect(container.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    expect(container).toHaveTextContent("Audit needs the Admin role");
  });
});

describe("PageSkeleton", () => {
  it("is a busy, named region that reserves the screen's shape", () => {
    render(<PageSkeleton label="Loading swimlanes" />);
    const region = screen.getByLabelText("Loading swimlanes");
    expect(region).toHaveAttribute("aria-busy", "true");
    expect(region.querySelectorAll("[data-slot='skeleton']").length).toBeGreaterThan(3);
  });
});
