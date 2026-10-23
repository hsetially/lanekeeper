import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Attribution, AuditEvent } from "@/api/generated/model";
import { EventWho } from "./EventWho";

const PRIYA = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const MARCO = "11111111-1111-4111-8111-111111111111:3f2a91bc-0000-4000-8000-000000000003";

const attribution = (over: Partial<Attribution> = {}): Attribution => ({ source: "tool_write", confidence: "certain", actor: { kind: "user", user: MARCO }, evidence: {}, ...over });
const event = (over: Partial<Pick<AuditEvent, "actor" | "attribution">> = {}): Pick<AuditEvent, "actor" | "attribution"> => ({ actor: PRIYA, attribution: attribution(), ...over });

describe("EventWho", () => {
  it("shows the person the attribution names, in bold", () => {
    render(<EventWho event={event()} nameOf={(id) => (id === MARCO ? "Marco Diaz" : undefined)} />);
    expect(screen.getByText("Marco Diaz").closest("b")).not.toBeNull();
    expect(screen.queryByText("probable")).not.toBeInTheDocument();
  });

  it("says You for the signed-in person when it is told who that is", () => {
    render(<EventWho event={event()} you={MARCO} />);
    expect(screen.getByText("You")).toBeInTheDocument();
  });

  it("says probable under a name the server is not sure of", () => {
    render(<EventWho event={event({ attribution: attribution({ source: "fs_owner_hint", confidence: "low", actor: { kind: "os_login", login: "svc-batch" } }) })} />);
    expect(screen.getByText("svc-batch")).toBeInTheDocument();
    expect(screen.getByText("probable")).toBeInTheDocument();
  });

  it("shows an unknown author as the dashed badge, and no name", () => {
    const { container } = render(<EventWho event={event({ actor: null, attribution: attribution({ source: "unknown", confidence: "low", actor: null }) })} />);
    expect(container.querySelector("[data-slot='unknown-author']")).toHaveTextContent("unknown author");
    expect(container.querySelector("b")).toBeNull();
  });

  it("shows Lanekeeper for an event nobody made", () => {
    render(<EventWho event={event({ actor: null, attribution: null })} />);
    expect(screen.getByText("Lanekeeper")).toBeInTheDocument();
  });

  it("never names a person it has no name for", () => {
    render(<EventWho event={event({ attribution: null })} />);
    expect(screen.getByText("Lanekeeper user 00000000")).toBeInTheDocument();
  });

  it("draws a hostile name as markers: a right-to-left override and a zero-width space are shown, not obeyed", () => {
    const { container } = render(<EventWho event={event()} nameOf={() => "Marco\u202e Di\u200baz"} />);
    expect(container.textContent).toContain("Marco<U+202E> Di<U+200B>az");
    expect(container.textContent).not.toContain("\u202e");
    expect(container.textContent).not.toContain("\u200b");
    expect(container.querySelector("bdi")).not.toBeNull();
  });
});
