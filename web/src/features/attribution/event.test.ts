import { describe, expect, it } from "vitest";
import type { Attribution, AuditEvent } from "@/api/generated/model";
import { eventWho } from "./event";

// Who made an audit event, for the User column of the audit log and the line under a row of the activity feed. The audit read
// carries the actor of the event as an id and, for a change to a file, an attribution that names who really made the change
// (the event's actor is then who recorded it, an approver for example). Nothing is guessed: a person is named by the attribution,
// else by the event's own actor, else the event says nobody known.

const PRIYA = "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const MARCO = "11111111-1111-4111-8111-111111111111:3f2a91bc-0000-4000-8000-000000000003";

const attribution = (over: Partial<Attribution> = {}): Attribution => ({ source: "tool_write", confidence: "certain", actor: { kind: "user", user: MARCO }, evidence: {}, ...over });
const event = (over: Partial<Pick<AuditEvent, "actor" | "attribution">> = {}): Pick<AuditEvent, "actor" | "attribution"> => ({ actor: PRIYA, attribution: attribution(), ...over });

const people = (id: string) => ({ [PRIYA]: "Priya Shah", [MARCO]: "Marco Diaz" })[id];

describe("eventWho", () => {
  it("names the person the attribution names, not the one who recorded the event", () => {
    expect(eventWho(event(), { nameOf: people })).toEqual({ kind: "named", label: "Marco Diaz", probable: false });
  });

  it("is You when the attribution names the signed-in person, if the screen asks for that", () => {
    expect(eventWho(event(), { you: MARCO, nameOf: people })).toEqual({ kind: "named", label: "You", probable: false });
    expect(eventWho(event(), { nameOf: people })).toMatchObject({ label: "Marco Diaz" });
  });

  it("names an OS login and a sync job by their own text", () => {
    expect(eventWho(event({ attribution: attribution({ source: "sentinel_login", confidence: "high", actor: { kind: "os_login", login: "dana.okoye" } }) }))).toEqual({ kind: "named", label: "dana.okoye", probable: false });
    expect(eventWho(event({ attribution: attribution({ source: "sync_job", confidence: "high", actor: { kind: "job", job: { name: "csp-tenant-data-sit7", uid: "u1" } } }) }))).toEqual({
      kind: "named",
      label: "Sync job csp-tenant-data-sit7",
      probable: false,
    });
  });

  it("marks a name as probable when the server is not sure: low or medium, never high or certain", () => {
    const probable = (confidence: Attribution["confidence"]) => (eventWho(event({ attribution: attribution({ source: "fs_owner_hint", confidence, actor: { kind: "os_login", login: "svc-batch" } }) })) as { probable: boolean }).probable;
    expect([probable("low"), probable("medium"), probable("high"), probable("certain")]).toEqual([true, true, false, false]);
  });

  it("is an unknown author when the source is unknown, and when the server saw a client or a job but named nobody", () => {
    expect(eventWho(event({ attribution: attribution({ source: "unknown", confidence: "low", actor: null }) }))).toEqual({ kind: "unknown" });
    expect(eventWho(event({ actor: null, attribution: attribution({ source: "nfs_client", confidence: "medium", actor: null }) }))).toEqual({ kind: "unknown" });
    expect(eventWho(event({ actor: null, attribution: attribution({ source: "sync_job", confidence: "high", actor: null }) }))).toEqual({ kind: "unknown" });
  });

  it("falls back to the event's own actor when the attribution is absent, or a tool write that names nobody", () => {
    expect(eventWho(event({ attribution: null }), { nameOf: people })).toEqual({ kind: "named", label: "Priya Shah", probable: false });
    expect(eventWho(event({ attribution: attribution({ source: "tool_write", actor: null }) }), { nameOf: people })).toEqual({ kind: "named", label: "Priya Shah", probable: false });
  });

  it("is the system when nobody is named anywhere", () => {
    expect(eventWho(event({ actor: null, attribution: null }))).toEqual({ kind: "system" });
  });

  it("never makes up a name: without a lookup a user is Lanekeeper user and the start of the id", () => {
    expect(eventWho(event({ attribution: null }))).toEqual({ kind: "named", label: "Lanekeeper user 00000000", probable: false });
    expect(eventWho(event())).toEqual({ kind: "named", label: "Lanekeeper user 3f2a91bc", probable: false });
  });
});
