// Audit: board frames 1v to 1ap of "Lanekeeper Approvals and Audit", and the States-index rows of Audit.
import { idsBetween, registerCoverage, registerFrames } from "../search/kit";
import { excluded, frames, states } from "./frames";

registerFrames("audit", [...frames, ...states]);
registerCoverage("audit", "approvals", idsBetween("approvals", "1v", "1ap"), frames, excluded);
