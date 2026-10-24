import type { ServiceRef } from "@/api/generated/model";
import { hasControlOrBackslash } from "@/lib/text/control";

// The checks the Admin forms make before they send. They mirror the contract's patterns and lengths (api/openapi.yaml), so a
// mistake is said next to the field; the hub checks again whatever they let through, and a refusal from it (422) is shown in the
// dialog. A check returns the message, or null when the value is fine. No message repeats the value it was given (a webhook URL
// is a secret).

/** ConfigKey and TenantId in the contract. */
const KEY = /^[a-z0-9][a-z0-9_-]{0,62}$/;
const KEY_HELP = "Use lower case letters, digits, - and _, starting with a letter or digit, at most 63 characters";
const K8S_NAME = /^[a-z0-9]([a-z0-9.-]{0,251}[a-z0-9])?$/;
const SERVICE_ACCOUNT = /^[a-z][a-z0-9-]*@[a-z][a-z0-9-]*\.iam\.gserviceaccount\.com$/;
const HOST_PATTERN = /^[A-Za-z0-9*._-]+$/;

export const MAX_DEPLOYMENTS = 100;

const tooLong = (value: string, max: number): string | null => (value.length > max ? `At most ${max.toLocaleString("en-US")} characters` : null);

export function checkConfigKey(value: string, empty = "Enter an id"): string | null {
  if (value === "") return empty;
  return KEY.test(value) ? null : KEY_HELP;
}

export function checkTenant(value: string): string | null {
  if (value === "") return "Enter a tenant";
  return KEY.test(value) ? null : KEY_HELP;
}

export function checkName(value: string): string | null {
  const trimmed = value.trim();
  if (trimmed === "") return "Enter a name";
  return tooLong(trimmed, 256);
}

function checkRelativePath(value: string, leadingSlash: string, empty: string): string | null {
  if (value === "") return empty;
  const long = tooLong(value, 1024);
  if (long !== null) return long;
  if (hasControlOrBackslash(value)) return "A path can't contain backslashes or control characters";
  if (value.startsWith("/")) return leadingSlash;
  if (value.split("/").includes("..")) return "A path can't contain .. segments";
  return null;
}

/** A root inside a Git repository (`RepoPath`): relative, no `..`. */
export function checkRepoRoot(value: string): string | null {
  return checkRelativePath(value, "Repo paths are relative. Remove the leading /", "Enter the repo path");
}

/** A folder below the swimlane's NFS root (`NfsPath`). */
export function checkFolder(value: string): string | null {
  const base = checkRelativePath(value, "NFS folders are relative to the swimlane's root. Remove the leading /", "Enter the NFS folder");
  if (base !== null) return base;
  return value.endsWith("/") ? "Write the folder without a trailing /" : null;
}

export function checkGlob(value: string): string | null {
  if (value.trim() === "") return "Enter a path glob";
  const long = tooLong(value, 256);
  if (long !== null) return long;
  if (/\s/.test(value)) return "Globs can't contain spaces";
  const open = value.split("[").length - 1;
  const close = value.split("]").length - 1;
  if (open !== close) return "Invalid glob: unclosed [";
  return null;
}

export function checkHostPattern(value: string): string | null {
  if (value === "") return "Enter a host pattern";
  const long = tooLong(value, 253);
  if (long !== null) return long;
  return HOST_PATTERN.test(value) ? null : "Use letters, digits, . - _ and * only: a host name, not an address with a scheme or a path";
}

export function checkServiceAccount(value: string): string | null {
  if (value === "") return "Enter the service account";
  if (value.length > 256) return "At most 256 characters";
  return SERVICE_ACCOUNT.test(value) ? null : "Must look like name@project.iam.gserviceaccount.com";
}

/** A Teams webhook is an https address. Nothing of the value is repeated in the message. */
export function checkWebhook(value: string): string | null {
  if (value === "") return "Enter the webhook URL";
  const long = tooLong(value, 2048);
  if (long !== null) return long;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return "Enter a full https:// address";
  }
  if (url.protocol !== "https:") return "A webhook must be an https:// address";
  if (url.username !== "" || url.password !== "") return "Remove the user name and password from the address";
  return null;
}

export type ParsedDeployments = { ok: true; value: ServiceRef[] } | { ok: false; error: string };

/** `namespace/name`, one per line or separated by commas. Repeats are dropped. */
export function parseDeployments(text: string): ParsedDeployments {
  const lines = text.split(/\r?\n/);
  const seen = new Set<string>();
  const value: ServiceRef[] = [];
  for (const [index, raw] of lines.entries()) {
    for (const part of raw.split(",")) {
      const entry = part.trim();
      if (entry === "") continue;
      const slash = entry.split("/");
      const [namespace, name] = slash;
      if (slash.length !== 2 || namespace === undefined || name === undefined || namespace === "" || name === "") return { ok: false, error: `Line ${index + 1}: write it as namespace/name` };
      if (!K8S_NAME.test(namespace) || !K8S_NAME.test(name)) return { ok: false, error: `Line ${index + 1}: lower case letters, digits, . and - only, starting and ending with a letter or digit` };
      if (seen.has(entry)) continue;
      seen.add(entry);
      value.push({ namespace, name });
    }
  }
  if (value.length === 0) return { ok: false, error: "Enter at least one Deployment as namespace/name" };
  if (value.length > MAX_DEPLOYMENTS) return { ok: false, error: `At most ${MAX_DEPLOYMENTS} Deployments` };
  return { ok: true, value };
}

/** `value` without one key: for the state of a form's errors, where a key is dropped and not set to undefined. */
export function omitKey<T extends object, K extends keyof T>(value: T, key: K): Omit<T, K> {
  const { [key]: _dropped, ...rest } = value;
  return rest;
}
