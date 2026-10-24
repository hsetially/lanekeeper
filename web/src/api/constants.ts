// Paths and names the app shares with the hub. src/api/constants.drift.test.ts reads the same values from the Rust source
// and from api/openapi.yaml, so a change on either side fails a test here instead of a user.

export const API_PREFIX = "/api/v1/";
export const AUTH_PREFIX = "/auth/";

/** The Server-Sent Events stream (T3). Its operation is not generated: see scripts/api/openapi-transformer.mjs. */
export const EVENTS_PATH = "/api/v1/events";

export const LOGIN_PATH = "/auth/login";
export const LOGOUT_PATH = "/auth/logout";

/** Where the hub's sign-in callback sends a user who has no role yet, and one who has been disabled (routes/auth.rs). */
export const ACCESS_PENDING_PATH = "/access-requested";
export const ACCESS_DISABLED_PATH = "/access-disabled";
