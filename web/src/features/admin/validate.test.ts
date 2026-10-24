import { describe, expect, it } from "vitest";
import { checkConfigKey, checkFolder, checkGlob, checkHostPattern, checkName, checkRepoRoot, checkServiceAccount, checkTenant, checkWebhook, parseDeployments } from "./validate";

// The checks the forms make before they send, so a mistake is said next to the field instead of as a refusal. They mirror the
// contract's patterns and lengths (api/openapi.yaml); the hub checks again whatever they let through.

describe("checkConfigKey", () => {
  it("accepts the contract's identifier shape", () => {
    for (const ok of ["a", "path-1", "default", "x_y", "9lives"]) expect(checkConfigKey(ok)).toBeNull();
  });
  it("refuses an empty one, capitals, spaces, a leading dash and more than 63 characters", () => {
    expect(checkConfigKey("")).toBe("Enter an id");
    for (const bad of ["Upper", "a b", "-lead", "_lead", "x".repeat(64)]) expect(checkConfigKey(bad)).toMatch(/lower case letters, digits, - and _/i);
  });
});

describe("checkRepoRoot", () => {
  it("accepts a relative path", () => {
    expect(checkRepoRoot("config")).toBeNull();
    expect(checkRepoRoot("data/config")).toBeNull();
  });
  it("refuses an empty path, an absolute path, a .. segment, a backslash and a control character", () => {
    expect(checkRepoRoot("")).toBe("Enter the repo path");
    expect(checkRepoRoot("/config")).toBe("Repo paths are relative. Remove the leading /");
    expect(checkRepoRoot("a/../b")).toBe("A path can't contain .. segments");
    expect(checkRepoRoot("a\\b")).toBe("A path can't contain backslashes or control characters");
    expect(checkRepoRoot("a\u0007b")).toBe("A path can't contain backslashes or control characters");
  });
  it("refuses more than 1,024 characters", () => {
    expect(checkRepoRoot("a".repeat(1025))).toBe("At most 1,024 characters");
  });
  it("refuses a look-alike for .. only when it is a whole segment", () => {
    expect(checkRepoRoot("a/..b/c")).toBeNull();
  });
});

describe("checkFolder", () => {
  it("is a relative NFS folder without a trailing slash", () => {
    expect(checkFolder("video")).toBeNull();
    expect(checkFolder("tx-infinity-api/core")).toBeNull();
    expect(checkFolder("")).toBe("Enter the NFS folder");
    expect(checkFolder("/video")).toBe("NFS folders are relative to the swimlane's root. Remove the leading /");
    expect(checkFolder("video/")).toBe("Write the folder without a trailing /");
    expect(checkFolder("../etc")).toBe("A path can't contain .. segments");
  });
});

describe("checkGlob", () => {
  it("accepts a glob", () => {
    expect(checkGlob("security/**")).toBeNull();
    expect(checkGlob("services/*/application-[a-z].yml")).toBeNull();
  });
  it("says what is wrong", () => {
    expect(checkGlob("  ")).toBe("Enter a path glob");
    expect(checkGlob("a/[b")).toBe("Invalid glob: unclosed [");
    expect(checkGlob("a b")).toBe("Globs can't contain spaces");
    expect(checkGlob("x".repeat(257))).toBe("At most 256 characters");
  });
});

describe("checkHostPattern", () => {
  it("accepts a host name with wildcards", () => {
    expect(checkHostPattern("*-dev.example.net")).toBeNull();
    expect(checkHostPattern("video.prod.example.net")).toBeNull();
  });
  it("refuses an empty pattern, spaces, a scheme or a path, and a pattern over 253 characters", () => {
    expect(checkHostPattern("")).toBe("Enter a host pattern");
    expect(checkHostPattern("a b.net")).toMatch(/letters, digits/i);
    expect(checkHostPattern("https://a.net")).toMatch(/letters, digits/i);
    expect(checkHostPattern("a.net/x")).toMatch(/letters, digits/i);
    expect(checkHostPattern("a".repeat(254))).toBe("At most 253 characters");
  });
});

describe("checkServiceAccount", () => {
  it("wants a Google service account address", () => {
    expect(checkServiceAccount("lanekeeper-agent@dfb-nonprod-sit.iam.gserviceaccount.com")).toBeNull();
    expect(checkServiceAccount("")).toBe("Enter the service account");
    expect(checkServiceAccount("agent@example.com")).toBe("Must look like name@project.iam.gserviceaccount.com");
    expect(checkServiceAccount("a@b.iam.gcp")).toBe("Must look like name@project.iam.gserviceaccount.com");
  });
});

describe("checkWebhook", () => {
  it("accepts an https address", () => {
    expect(checkWebhook("https://corp.webhook.office.com/webhookb2/abc@def/IncomingWebhook/123/456")).toBeNull();
  });
  it("refuses an empty value, something that is not a URL, plain http, and credentials in the address", () => {
    expect(checkWebhook("")).toBe("Enter the webhook URL");
    expect(checkWebhook("not a url")).toBe("Enter a full https:// address");
    expect(checkWebhook("http://corp.webhook.office.com/x")).toBe("A webhook must be an https:// address");
    expect(checkWebhook("https://user:pw@corp.webhook.office.com/x")).toBe("Remove the user name and password from the address");
    expect(checkWebhook(`https://a.example/${"x".repeat(2100)}`)).toBe("At most 2,048 characters");
  });
  it("never repeats the value it was given in its message", () => {
    const secret = "https://corp.webhook.office.com/webhookb2/TOPSECRET";
    for (const bad of ["not a url", `http://${secret.slice(8)}`, `https://u:p@${secret.slice(8)}`]) expect(checkWebhook(bad) ?? "").not.toContain("TOPSECRET");
  });
});

describe("checkName and checkTenant", () => {
  it("wants a name of 1 to 256 characters", () => {
    expect(checkName("Approvals")).toBeNull();
    expect(checkName("  ")).toBe("Enter a name");
    expect(checkName("x".repeat(257))).toBe("At most 256 characters");
  });
  it("wants a tenant id in the contract's shape (a Git branch name)", () => {
    expect(checkTenant("sit7")).toBeNull();
    expect(checkTenant("")).toBe("Enter a tenant");
    expect(checkTenant("SIT 7")).toMatch(/lower case letters, digits, - and _/i);
  });
});

describe("parseDeployments", () => {
  it("reads one namespace/name per line, or separated by commas", () => {
    expect(parseDeployments("sitb/video-service\nsitb/tx-core")).toEqual({ ok: true, value: [{ namespace: "sitb", name: "video-service" }, { namespace: "sitb", name: "tx-core" }] });
    expect(parseDeployments("sitb/a, sitc/b")).toEqual({ ok: true, value: [{ namespace: "sitb", name: "a" }, { namespace: "sitc", name: "b" }] });
  });
  it("ignores blank lines and repeats", () => {
    expect(parseDeployments("\nsitb/a\n\nsitb/a\n")).toEqual({ ok: true, value: [{ namespace: "sitb", name: "a" }] });
  });
  it("refuses none, a line without a slash, and a name Kubernetes would refuse, naming the line", () => {
    expect(parseDeployments("  \n")).toEqual({ ok: false, error: "Enter at least one Deployment as namespace/name" });
    expect(parseDeployments("sitb/a\nvideo")).toEqual({ ok: false, error: "Line 2: write it as namespace/name" });
    expect(parseDeployments("sitb/Video")).toEqual({ ok: false, error: "Line 1: lower case letters, digits, . and - only, starting and ending with a letter or digit" });
  });
  it("refuses more than 100", () => {
    const many = Array.from({ length: 101 }, (_, i) => `ns/svc-${i}`).join("\n");
    expect(parseDeployments(many)).toEqual({ ok: false, error: "At most 100 Deployments" });
  });
});
