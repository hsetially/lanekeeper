// Tier 1, the screens before sign-in: board "Access and Onboarding" frames 1a to 1f (sign in, access requested, access disabled) against the
// app on `design-access-public`, 1280x640, light and dark. There is no shell around these screens in either.
//
// Sign in is pixel for pixel the design's (0.000%). Access requested and access disabled lose rows the contract cannot fill (DEVIATIONS "Sign in,
// Access requested, Access disabled": "Signed in as", "Request sent <time>", the disabled date and the admin's name), and the card is centred
// in both, so what is under the missing rows moves: the card itself is masked (the two cards' boxes), and what each draws the same is compared
// where each is: the badge, the title and the sentence at the top (the app's card is shorter, so its top is lower), and the Sign out link.
import { tier1, type FrameCase } from "./lib/matrix";
import type { Aligned, Mask } from "./lib/compare";

const BOARD = "Lanekeeper Access and Onboarding.dc.html";
// The union of the design's card and the app's, so that neither is compared as a whole: x 439 to 841, y 157 to 484.
const CARD: Mask = { x: 439, y: 157, width: 402, height: 327, why: "Sign in, Access requested, Access disabled: rows the contract cannot fill move what follows them" };
const requestedTop: Aligned = { ref: { x: 473, y: 191, width: 334, height: 130 }, app: { x: 473, y: 236, width: 334, height: 130 }, why: "the badge, the title and the sentence" };
const requestedLink: Aligned = { ref: { x: 473, y: 430, width: 60, height: 22 }, app: { x: 473, y: 385, width: 60, height: 22 }, why: "the Sign out link" };
const disabledTop: Aligned = { ref: { x: 473, y: 193, width: 334, height: 52 }, app: { x: 473, y: 214, width: 334, height: 52 }, why: "the badge and the title" };
const disabledLink: Aligned = { ref: { x: 473, y: 428, width: 60, height: 22 }, app: { x: 473, y: 406, width: 60, height: 22 }, why: "the Sign out link" };

const cases: FrameCase[] = [
  { name: "sign in", board: BOARD, ids: ["1a", "1b"], scenario: "design-access-public", path: "/signin" },
  { name: "access requested", board: BOARD, ids: ["1c", "1d"], scenario: "design-access-public", path: "/access-requested", masks: [CARD], aligned: [requestedTop, requestedLink] },
  { name: "access disabled", board: BOARD, ids: ["1e", "1f"], scenario: "design-access-public", path: "/access-disabled", masks: [CARD], aligned: [disabledTop, disabledLink] },
];
tier1(cases);
