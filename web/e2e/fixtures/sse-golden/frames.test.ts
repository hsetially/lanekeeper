import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import Ajv2020 from "ajv/dist/2020.js";
import { parse } from "yaml";

// The golden frames must be events the contract allows: each `data:` line is a DomainEvent, and `event:` is its type.
const dir = new URL("./", import.meta.url);
const doc = parse(readFileSync(new URL("../../../../api/openapi.yaml", import.meta.url), "utf8")) as object;
const ajv = new Ajv2020({ strict: false, validateFormats: false });
ajv.addSchema(doc, "oas");
const validate = ajv.compile({ $ref: "oas#/components/schemas/DomainEvent" });

const frames = readdirSync(dir).filter((f) => f.endsWith(".txt"));

describe("golden SSE frames", () => {
  it("has the six frames the README names", () => {
    expect(frames.sort()).toEqual(["file-observed.txt", "final-retry.txt", "findings-changed.txt", "first-frame.txt", "heartbeat.txt", "resync.txt"]);
  });

  it.each(frames)("%s is one well-formed frame", (name) => {
    const text = readFileSync(new URL(name, dir), "utf8");
    expect(text.endsWith("\n\n")).toBe(true);
    expect(text.includes("\r")).toBe(false);
  });

  it.each(frames.filter((f) => /file-observed|findings-changed|resync/.test(f)))("%s carries a DomainEvent of the named type", (name) => {
    const text = readFileSync(new URL(name, dir), "utf8");
    const fields = Object.fromEntries(text.trimEnd().split("\n").map((l) => [l.slice(0, l.indexOf(":")), l.slice(l.indexOf(":") + 2)]));
    const event = JSON.parse(fields["data"] ?? "null") as { type: string };
    expect(validate(event), JSON.stringify(validate.errors)).toBe(true);
    expect(fields["event"]).toBe(event.type);
    if (name.startsWith("resync")) expect(fields["id"]).toBeUndefined();
    else expect(fields["id"]).toMatch(/^[1-9][0-9]*$/);
  });
});
