import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Attribution } from "@/api/generated/model";
import { FileChangedBy } from "./FileChangedBy";

const ME = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";

const attribution = (over: Partial<Attribution> = {}): Attribution => ({ source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" }, evidence: {}, ...over });

describe("FileChangedBy, the head of a file", () => {
  it("says who changed the file the way a history entry does, after the words Changed by", () => {
    const { container } = render(<FileChangedBy attribution={attribution()} />);
    expect(container.querySelector("[data-slot='file-changed-by']")).toHaveTextContent("Changed by");
    expect(screen.getByRole("button")).toHaveTextContent("dana.okoye");
    expect(screen.getByRole("button")).toHaveTextContent("edited on the NFS VM");
    expect(screen.getByRole("button")).toHaveTextContent("Confidence: High");
  });

  it("says Unknown author for a file nobody was identified for, and does not say Low", () => {
    const { container } = render(<FileChangedBy attribution={attribution({ source: "unknown", confidence: "low", actor: null })} />);
    expect(container).toHaveTextContent("Changed by");
    expect(screen.getByRole("button")).toHaveTextContent("Unknown author");
    expect(container).not.toHaveTextContent("Low");
  });

  it("says that nothing is recorded when the hub has no attribution for the file, and invents no author", () => {
    const { container } = render(<FileChangedBy attribution={null} />);
    expect(container).toHaveTextContent("Changed by");
    expect(screen.getByText("Not recorded")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("takes another label, and the signed-in person from the screen", () => {
    render(<FileChangedBy attribution={attribution({ source: "tool_write", confidence: "certain", actor: { kind: "user", user: ME } })} label="Last change" you={ME} />);
    expect(screen.getByText("Last change")).toBeInTheDocument();
    expect(screen.getByRole("button")).toHaveTextContent("You");
  });
});
