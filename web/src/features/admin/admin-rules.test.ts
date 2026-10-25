import { describe, expect, it } from "vitest";
import { OPERATION_ROLES } from "@/api/roles.generated";
import { productSources, withoutComments } from "@/test/source-files";

// Rules of the Admin area that the lint cannot see, read from the source (an approximation, and it says so): what is secret goes
// only through the secret path, what is shown before the hub answers is only the auto-notify switch, and nothing here copies, stores
// or logs. A file that breaks one fails by name.

const files = () => productSources().filter((f) => f.path.startsWith("src/features/admin/")).map((f) => ({ path: f.path, text: withoutComments(f.text) }));

describe("the Admin area's source", () => {
  it("has files to read", () => {
    expect(files().length).toBeGreaterThan(20);
  });

  it("is Admin only on the hub's side for every operation it calls, by the generated role map (x-required-role)", () => {
    const admin = Object.entries(OPERATION_ROLES).filter(([id]) => id.startsWith("admin"));
    expect(admin.length).toBeGreaterThan(30);
    for (const [id, operation] of admin) expect(operation.role, id).toBe("admin");
    // The one other thing the area calls is the signed-in person's own token list, which is not an Admin operation.
    expect(OPERATION_ROLES.listMcpTokens.role).toBe("viewer");
  });

  it("copies only the join token, with the secret copy button: the generic CopyButton is never imported here, and nothing else in the Admin area is copied", () => {
    expect(files().filter((f) => /from\s+["'][^"']*\/CopyButton["']/.test(f.text)).map((f) => f.path)).toEqual([]);
    expect(files().filter((f) => /SecretCopyButton/.test(f.text)).map((f) => f.path)).toEqual(["src/features/admin/swimlanes/JoinTokenCard.tsx"]);
  });

  it("shows a result before the hub answers for the auto-notify switch only", () => {
    const users = files().filter((f) => /optimisticWriteMutation|setAutoNotify/.test(f.text)).map((f) => f.path).sort();
    expect(users).toEqual(["src/features/admin/mutations.ts", "src/features/admin/swimlanes/AutoNotifySwitch.tsx"]);
  });

  it("keeps the two secret-bearing requests for useSecretMutation: a webhook URL and a join token", () => {
    for (const request of ["putNotificationRequest", "issueJoinTokenRequest"]) {
      const using = files().filter((f) => f.path !== "src/features/admin/mutations.ts" && f.text.includes(request));
      expect(using.length, request).toBeGreaterThan(0);
      for (const f of using) expect(f.text, `${f.path} must pass ${request} to useSecretMutation`).toMatch(/useSecretMutation\(/);
    }
    // And the two operations are not in the table of plain mutations.
    const mutations = files().find((f) => f.path === "src/features/admin/mutations.ts")?.text ?? "";
    expect(mutations).not.toMatch(/writeMutation<"adminPutNotification"/);
    expect(mutations).not.toMatch(/writeMutation<"adminIssueJoinToken"/);
  });

  it("builds every plain mutation from adminMutations, so each write is invalidated by the table and never retried", () => {
    for (const f of files().filter((x) => /useMutation\(/.test(x.text))) {
      for (const call of f.text.matchAll(/useMutation\(([^)]*)\)/g)) expect(call[1], `${f.path}: ${call[0]}`).toMatch(/^adminMutations\./);
    }
  });

  it("stores nothing, logs nothing and writes no HTML", () => {
    const banned = /localStorage|sessionStorage|indexedDB|document\.cookie|console\.|dangerouslySetInnerHTML|innerHTML|setQueryData|onMutate|\bfetch\(/;
    expect(files().filter((f) => banned.test(f.text)).map((f) => f.path)).toEqual([]);
  });

  it("uses no hand-typed query key: every key comes from the factories", () => {
    expect(files().filter((f) => /queryKey:\s*\[/.test(f.text)).map((f) => f.path)).toEqual([]);
  });
});
