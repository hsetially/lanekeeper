import { describe, expect, it } from "vitest";
import type { GithubCredential } from "@/api/generated/model";
import { expiryWords, tokenHealth } from "./expiry";

const DAY = 86_400_000;
const NOW = 1_760_000_000_000;
const credential = (over: Partial<GithubCredential> = {}): GithubCredential => ({ github_login: "mdiaz", verified_at: NOW - DAY, scopes: [], ...over });

describe("expiryWords", () => {
  it("counts whole days, rounding up, so a token never reads as having more time than it has", () => {
    expect(expiryWords(NOW + 5 * DAY, NOW)).toBe("in 5 days");
    expect(expiryWords(NOW + 4 * DAY + 1000, NOW)).toBe("in 5 days");
    expect(expiryWords(NOW + 90 * DAY, NOW)).toBe("in 90 days");
  });

  it("says one day in the singular and counts the last hours as one day", () => {
    expect(expiryWords(NOW + DAY, NOW)).toBe("in 1 day");
    expect(expiryWords(NOW + 3_600_000, NOW)).toBe("in 1 day");
  });

  it("says expired for a time that is now or has passed", () => {
    expect(expiryWords(NOW, NOW)).toBe("expired");
    expect(expiryWords(NOW - DAY, NOW)).toBe("expired");
  });
});

describe("tokenHealth", () => {
  it("is expired once the time has come, whatever the hub's flag says", () => {
    expect(tokenHealth(credential({ expires_at: NOW - 1 }), NOW)).toBe("expired");
    expect(tokenHealth(credential({ expires_at: NOW - 1, expires_soon: false }), NOW)).toBe("expired");
  });

  it("is soon when the hub says the token ends within seven days", () => {
    expect(tokenHealth(credential({ expires_at: NOW + 5 * DAY, expires_soon: true }), NOW)).toBe("soon");
  });

  it("is fine when there is time left, and when GitHub gave no expiry at all", () => {
    expect(tokenHealth(credential({ expires_at: NOW + 60 * DAY, expires_soon: false }), NOW)).toBe("ok");
    expect(tokenHealth(credential({ expires_at: null }), NOW)).toBe("ok");
    expect(tokenHealth(credential(), NOW)).toBe("ok");
  });
});
