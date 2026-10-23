// A small hub for the tests of the four sections that task T5 adds to the Admin area: the Admin world of S8 (who is signed in, the
// people, the swimlanes, the rules) with the sentinels, the OS Login map, the severity rules and the retention policy on top. A write
// changes the world the way the hub would, so a list asked for again after the write shows the new state; a test can also make a call
// fail, or hold it, to play the states the hub gives in bad moments. Everything else is answered by the S8 world.
// Test code only: nothing outside the tests imports it.
import type { OsLoginUser, RetentionInput, RetentionSettings, SentinelInfo, SeverityRule, SeverityRuleInput } from "../../../api/generated/model/index.ts";
import { json, problem, type Api } from "../../../test/app.tsx";
import { newWorld, worldApi, type World } from "../admin/world.ts";
import { NOW, osLoginUsers, previewFor, retention, sentinels, severityRules } from "./fixtures.ts";

export type T5World = World & {
  sentinels: SentinelInfo[];
  osLogin: OsLoginUser[];
  severity: SeverityRule[];
  retention: RetentionSettings;
};

export function newT5World(role: World["role"] = "admin"): T5World {
  return { ...newWorld(role), sentinels: sentinels(), osLogin: osLoginUsers(), severity: severityRules(), retention: retention() };
}

const page = (items: readonly unknown[]): Response => json({ items, next: null });
const noContent = (): Response => new Response(null, { status: 204 });

function bodyOf(text: string | undefined): Record<string, unknown> {
  try {
    const parsed: unknown = text === undefined ? {} : JSON.parse(text);
    return typeof parsed === "object" && parsed !== null ? (parsed as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** Replace the entry with this key, or add one (what a PUT of a rule does). */
function upsert<T>(list: T[], item: T, same: (x: T) => boolean): T[] {
  return list.some(same) ? list.map((x) => (same(x) ? item : x)) : [...list, item];
}

export function t5Api(world: T5World): Api {
  const base = worldApi(world);
  return (call) => {
    const url = new URL(call.path, "http://hub.test");
    const path = url.pathname;
    for (const [index, override] of world.overrides.entries()) {
      if (override.match(call)) {
        if (override.once === true) world.overrides.splice(index, 1);
        return override.respond(call);
      }
    }
    if (world.hold.has(path)) return new Promise<Response>(() => undefined);

    if (path === "/api/v1/admin/sentinels" && call.method === "GET") return page(world.sentinels);

    if (path === "/api/v1/admin/os-login-users" && call.method === "GET") return page(world.osLogin);
    const login = /^\/api\/v1\/admin\/os-login-users\/([^/]+)$/.exec(path)?.[1];
    if (login !== undefined) {
      const name = decodeURIComponent(login);
      if (call.method === "DELETE") {
        if (!world.osLogin.some((m) => m.os_login === name)) return problem(404, "not_found");
        world.osLogin = world.osLogin.filter((m) => m.os_login !== name);
        return noContent();
      }
      if (call.method === "PUT") {
        const user = bodyOf(call.body)["user"];
        if (typeof user !== "string" || !world.users.some((u) => u.id === user)) return problem(422, "unprocessable");
        const mapping: OsLoginUser = { os_login: name, user };
        world.osLogin = upsert(world.osLogin, mapping, (m) => m.os_login === name);
        return json(mapping);
      }
    }

    if (path === "/api/v1/admin/severity-rules" && call.method === "GET") return page(world.severity);
    const ruleId = /^\/api\/v1\/admin\/severity-rules\/([^/]+)$/.exec(path)?.[1];
    if (ruleId !== undefined) {
      const id = decodeURIComponent(ruleId);
      if (call.method === "DELETE") {
        if (!world.severity.some((r) => r.id === id)) return problem(404, "not_found");
        world.severity = world.severity.filter((r) => r.id !== id);
        return noContent();
      }
      if (call.method === "PUT") {
        const input = bodyOf(call.body) as unknown as SeverityRuleInput;
        const rule: SeverityRule = { id, ...input };
        world.severity = upsert(world.severity, rule, (r) => r.id === id);
        return json(rule);
      }
    }

    if (path === "/api/v1/admin/retention") {
      if (call.method === "GET") return json(world.retention);
      if (call.method === "PUT") {
        const input = bodyOf(call.body) as unknown as RetentionInput;
        const preview = previewFor(input.keep_days, input.keep_last_versions);
        // A dry run changes nothing: the answer is the saved policy with the preview of the one that was sent.
        if (url.searchParams.get("dry_run") === "true") return json({ ...world.retention, dry_run_preview: preview });
        world.retention = { keep_days: input.keep_days, keep_last_versions: input.keep_last_versions, updated_at: NOW, updated_by: world.meId, dry_run_preview: preview };
        return json(world.retention);
      }
    }
    return base(call);
  };
}
