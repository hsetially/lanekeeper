import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { linkTo } from "@/routes/links";
import { CopyLink } from "./CopyLink";
import { absoluteLink } from "./copy-link";

// A permalink is built from a path `linkTo` made (never concatenated by hand) and the page's own origin. A path that is not the
// app's own is not a link we copy.

describe("absoluteLink", () => {
  it("joins an app path to this page's origin", () => {
    expect(absoluteLink("/swimlanes/sit1/files?path=a.yml&line=12")).toBe(`${window.location.origin}/swimlanes/sit1/files?path=a.yml&line=12`);
  });

  it("adds a fragment through the URL, so it is encoded and nothing in it can end the address", () => {
    expect(absoluteLink("/docs/view?path=a.md", "doc-rollout")).toBe(`${window.location.origin}/docs/view?path=a.md#doc-rollout`);
    expect(absoluteLink("/docs/view?path=a.md", "a b#c")).toBe(`${window.location.origin}/docs/view?path=a.md#a%20b#c`);
    expect(absoluteLink("/docs/view?path=a.md", "")).toBe(`${window.location.origin}/docs/view?path=a.md`);
  });

  it.each(["//evil.example/x", "https://evil.example/x", "javascript:alert(1)", "swimlanes", "", "/\\evil.example", "/a\nb", "/a b"])("refuses %j", (href) => {
    expect(absoluteLink(href)).toBeNull();
  });
});

describe("CopyLink", () => {
  it("copies the permalink of a route, and says what it is a link to", async () => {
    const user = userEvent.setup();
    render(<CopyLink href={linkTo("files", { id: "sit1" }, { path: "tx/core.yml", line: "12" })} what="link to this line" />);
    await user.click(screen.getByRole("button", { name: "Copy link to this line" }));
    expect(await navigator.clipboard.readText()).toBe(`${window.location.origin}/swimlanes/sit1/files?line=12&path=tx%2Fcore.yml`);
  });

  it("renders nothing for a path that is not the app's", () => {
    const { container } = render(<CopyLink href="https://evil.example/" what="link" />);
    expect(container).toBeEmptyDOMElement();
  });
});
