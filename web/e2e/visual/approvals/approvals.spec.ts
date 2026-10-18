// Approvals: board frames 1a to 1u of "Lanekeeper Approvals and Audit", and the States-index rows of Approvals.
import { idsBetween, registerCoverage, registerFrames } from "../search/kit";
import { excluded, frames, states } from "./frames";

registerFrames("approvals", [...frames, ...states]);
registerCoverage("approvals", "approvals", idsBetween("approvals", "1a", "1u"), frames, excluded);
