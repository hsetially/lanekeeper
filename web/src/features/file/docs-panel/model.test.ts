import { describe, expect, it } from "vitest";
import type { FlagStatus } from "@/api/generated/model";
import { docLink, flagRow } from "./model";

describe("docLink", () => {
  it("opens the reader at the doc, with the path encoded once", () => {
    expect(docLink("/git/main/docs/account-sorting.md", null)).toBe("/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Faccount-sorting.md");
    expect(docLink("/uploads/runbook.pdf", undefined)).toBe("/docs/view?path=%2Fuploads%2Frunbook.pdf");
  });

  it("adds the heading the server named, when it has the shape of a heading id", () => {
    expect(docLink("/git/main/docs/a.md", "doc-account-sorting")).toBe("/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Fa.md#doc-account-sorting");
  });

  it("drops an anchor that is not a heading id, and one that is too long", () => {
    expect(docLink("/git/main/docs/a.md", "Account Sorting")).toBe("/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Fa.md");
    expect(docLink("/git/main/docs/a.md", "javascript:alert(1)")).toBe("/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Fa.md");
    expect(docLink("/git/main/docs/a.md", `doc-${"a".repeat(80)}`)).toBe("/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Fa.md");
  });

  it("does not decode a percent escape: it stays text and is encoded again", () => {
    expect(docLink("/git/main/docs/a%2Fb.md", null)).toBe("/docs/view?path=%2Fgit%2Fmain%2Fdocs%2Fa%252Fb.md");
    expect(docLink("/git/main/%2e%2e/secret.md", null)).toBe("/docs/view?path=%2Fgit%2Fmain%2F%252e%252e%2Fsecret.md");
  });

  it.each(["", "docs/a.md", "/etc/passwd", "/git/", "/git/main//a.md", "/git/main/../a.md", "/git/main/./a.md", "/uploads/", "/git/main/a\\b.md", "/git/main/a\nb.md", "/git/main/a‮b.md", `/git/${"a".repeat(1100)}`, "https://evil.example/x.md"])(
    "makes no link of %j",
    (path) => {
      expect(docLink(path, "doc-x")).toBeNull();
    },
  );
});

const flag = (over: Partial<FlagStatus> = {}): FlagStatus => ({
  flag: "enableAccountSorting",
  file: "tx-infinity-api/tx-infinity-core.yml",
  channel: null,
  setting: "txInfinityOptions.enableAccountSorting",
  template_default: { text: "false", ty: "bool", flag: null },
  values: [{ swimlane: "sitb", value: { text: "true", ty: "bool", flag: null } }],
  ...over,
});

describe("flagRow", () => {
  it("takes the value this swimlane has, and says it differs from the template default", () => {
    expect(flagRow(flag(), "sitb")).toEqual({ setting: "txInfinityOptions.enableAccountSorting", flag: "enableAccountSorting", value: "true", fallback: "false", differs: true });
  });

  it("does not say it differs when it is the default", () => {
    expect(flagRow(flag({ values: [{ swimlane: "sitb", value: { text: "false", ty: "bool", flag: null } }] }), "sitb").differs).toBe(false);
  });

  it("has no value for a swimlane the flag has no value in, or one it does not set", () => {
    expect(flagRow(flag(), "sitc").value).toBeNull();
    expect(flagRow(flag({ values: [{ swimlane: "sitb", value: null }] }), "sitb").value).toBeNull();
  });

  it("is a difference to have no value where the template has one", () => {
    expect(flagRow(flag({ values: [] }), "sitb").differs).toBe(true);
  });

  it("is no difference when neither has a value", () => {
    expect(flagRow(flag({ values: [], template_default: null }), "sitb").differs).toBe(false);
  });
});
