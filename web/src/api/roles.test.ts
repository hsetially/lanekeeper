import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { parse } from "yaml";
import { Role } from "./generated/model";
import { OPERATION_ROLES, type OperationRole } from "./roles.generated";

const doc = parse(readFileSync(new URL("../../../api/openapi.yaml", import.meta.url), "utf8")) as {
  paths: Record<string, Record<string, { operationId?: string; "x-required-role"?: string }>>;
};
const operations = Object.entries(doc.paths).flatMap(([path, item]) =>
  Object.entries(item)
    .filter(([method]) => ["get", "post", "put", "patch", "delete"].includes(method))
    .map(([method, op]) => ({ path, method: method.toUpperCase(), id: op.operationId ?? "", role: op["x-required-role"] })),
);

describe("the generated role map", () => {
  it("has every operation of the contract, with its method, path and role", () => {
    expect(Object.keys(OPERATION_ROLES).sort()).toEqual(operations.map((o) => o.id).sort());
    for (const op of operations) {
      expect(OPERATION_ROLES[op.id as keyof typeof OPERATION_ROLES], op.id).toEqual({ method: op.method, path: op.path, role: op.role });
    }
  });

  it("gives every write a role (a write with 'none' is one of the four open operations and says so)", () => {
    const open = operations.filter((o) => o.role === "none").map((o) => o.id);
    expect(open.sort()).toEqual(["authCallback", "authLogin", "authLogout", "githubWebhook"].sort());
    for (const op of operations.filter((o) => o.method !== "GET")) expect(op.role, op.id).toBeTypeOf("string");
  });

  it("uses only roles the API knows", () => {
    const known = new Set<OperationRole>(["none", ...Object.values(Role)]);
    for (const [id, row] of Object.entries(OPERATION_ROLES)) expect(known.has(row.role), id).toBe(true);
  });
});
