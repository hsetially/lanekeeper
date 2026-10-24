import { describe, expect, it } from "vitest";
import { CSRF_COOKIE, CSRF_HEADER, readCsrfToken } from "./index";

const TOKEN = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde"; // 43 characters of base64url, as the hub issues

describe("readCsrfToken", () => {
  it("reads the double-submit cookie from a cookie string", () => {
    expect(readCsrfToken(`${CSRF_COOKIE}=${TOKEN}`)).toBe(TOKEN);
    expect(readCsrfToken(`a=1; ${CSRF_COOKIE}=${TOKEN}; b=2`)).toBe(TOKEN);
  });

  it("returns nothing when the cookie is absent or empty", () => {
    expect(readCsrfToken("")).toBeUndefined();
    expect(readCsrfToken("a=1; b=2")).toBeUndefined();
    expect(readCsrfToken(`${CSRF_COOKIE}=`)).toBeUndefined();
  });

  it("does not match a cookie whose name only contains ours", () => {
    expect(readCsrfToken(`x${CSRF_COOKIE}=${TOKEN}`)).toBeUndefined();
    expect(readCsrfToken(`${CSRF_COOKIE}x=${TOKEN}`)).toBeUndefined();
    expect(readCsrfToken(`lk_csrf=${TOKEN}`)).toBeUndefined();
  });

  it("believes none of two cookies with our name, as the server does", () => {
    expect(readCsrfToken(`${CSRF_COOKIE}=${TOKEN}; ${CSRF_COOKIE}=${TOKEN}`)).toBeUndefined();
  });

  it("refuses a value that is not base64url text, so it can never break a header", () => {
    for (const bad of ["short", `${TOKEN}\r\nX: y`, `${TOKEN} extra`, "a".repeat(300), `"${TOKEN}"`, `${TOKEN}%0d`]) {
      expect(readCsrfToken(`${CSRF_COOKIE}=${bad}`), bad).toBeUndefined();
    }
  });

  it("uses the names the hub uses", () => {
    expect(CSRF_COOKIE).toBe("__Host-lk_csrf");
    expect(CSRF_HEADER.toLowerCase()).toBe("x-csrf-token");
  });
});
