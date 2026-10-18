import { describe, expect, it } from "vitest";
import type { Proposal } from "@/api/generated/model";
import { bannerOf, otherAdminsSentence, type Answer } from "./banner";
import { proposal } from "@/mocks/design/approvals/fixtures";

const ME = "11111111-1111-4111-8111-111111111111:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const OTHER = "11111111-1111-4111-8111-111111111111:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const NOW = 1_760_000_000_000;
const names: Record<string, string> = { [ME]: "Priya Shah", [OTHER]: "Amir Haddad" };
const name = (id: string | null): string => (id === null ? "Lanekeeper" : (names[id] ?? "someone"));

const base = { me: ME, name, others: ["Amir Haddad", "Lena Brandt"], now: NOW, answer: null, sawPending: true } as const;
const of = (p: Partial<Proposal>, over: Partial<Parameters<typeof bannerOf>[0]> = {}) => bannerOf({ ...base, p: proposal({ id: 1, author: OTHER, swimlane: "sitb", path: "a/b.yml", ...p }), ...over });

describe("the banner on top of the drawer", () => {
  it("says nothing about an ordinary pending proposal", () => {
    expect(of({})).toBeNull();
  });

  it("locks your own proposal, and says who else can decide", () => {
    expect(of({ author: ME })).toEqual({
      variant: "lock",
      title: "You can't approve your own proposal",
      body: "Another Admin has to approve it, so no one can change a swimlane alone. Amir Haddad and Lena Brandt are the other Admins.",
      request: null,
    });
  });

  it("says a stale proposal can not be approved and what to do instead", () => {
    const banner = of({ status: "stale" });
    expect(banner?.variant).toBe("warning");
    expect(banner?.title).toBe("The file changed since this was proposed");
    expect(banner?.body).toBe("Approving would overwrite the newer file. Reject this proposal with a reason, and Amir Haddad can propose it again against the current file.");
  });

  it("says an expired proposal wrote nothing", () => {
    const banner = of({ status: "expired", expires_at: NOW - 6 * 3_600_000 });
    expect(banner?.variant).toBe("info");
    expect(banner?.title).toBe("Expired 6 h ago with no decision");
    expect(banner?.body).toBe("Nothing was written to sitb. Amir Haddad can propose it again.");
  });

  it("says who approved, when, and what happened to the file", () => {
    const banner = of({ status: "applied", decided_by: OTHER, decided_at: NOW - 120_000 }, { sawPending: false });
    expect(banner).toEqual({ variant: "success", title: "Approved by Amir Haddad, 2 min ago", body: "Written to sitb on approval.", request: null });
  });

  it("says that someone else decided while you had it open", () => {
    const banner = of({ status: "applied", decided_by: OTHER, decided_at: NOW - 8_000 }, { sawPending: true });
    expect(banner?.body).toBe("Someone else decided while you had it open. Written to sitb on approval.");
  });

  it("does not say that of your own decision", () => {
    const banner = of({ status: "applied", decided_by: ME, decided_at: NOW - 8_000 }, { sawPending: true });
    expect(banner?.title).toBe("Approved by Priya Shah, just now");
    expect(banner?.body).toBe("Written to sitb on approval.");
  });

  it("quotes the reason of a rejection, or says nothing was written", () => {
    expect(of({ status: "rejected", decided_by: OTHER, decided_at: NOW - 60_000, decision_comment: "Wrong swimlane." })).toEqual({
      variant: "danger",
      title: "Rejected by Amir Haddad, 1 min ago",
      body: "“Wrong swimlane.”",
      request: null,
    });
    expect(of({ status: "rejected", decided_by: OTHER, decision_comment: null })?.body).toBe("Nothing was written to sitb.");
  });

  it("gives the server's answer to your own approval before anything it derives", () => {
    const applied: Answer = { kind: "approved", audit: 18_470, warnings: 0 };
    expect(of({ status: "applied", decided_by: ME }, { answer: applied })).toEqual({ variant: "success", title: "Approved · written to sitb", body: "Audit event 18,470.", request: null });
    const warned: Answer = { kind: "approved", audit: 7, warnings: 2 };
    expect(of({}, { answer: warned })?.body).toBe("Audit event 7. The server noted 2 things worth knowing.");
    const sent: Answer = { kind: "sent", id: 2051 };
    expect(of({}, { answer: sent })).toEqual({
      variant: "info",
      title: "Sent for approval",
      body: "Your approval is not applied until another Admin approves request 2051.",
      request: 2051,
    });
  });
});

describe("the sentence about the other Admins", () => {
  it("reads as a list", () => {
    expect(otherAdminsSentence([])).toBe("");
    expect(otherAdminsSentence(["Amir Haddad"])).toBe("Amir Haddad is the other Admin.");
    expect(otherAdminsSentence(["Amir Haddad", "Lena Brandt"])).toBe("Amir Haddad and Lena Brandt are the other Admins.");
    expect(otherAdminsSentence(["A", "B", "C"])).toBe("A, B and C are the other Admins.");
    expect(otherAdminsSentence(["A", "B", "C", "D", "E", "F"])).toBe("A, B, C, D and 2 more are the other Admins.");
  });
});
