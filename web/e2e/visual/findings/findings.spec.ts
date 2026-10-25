// Findings: board frames 1a to 1t of "Lanekeeper Findings and Adoption" (the adoption wizard's frames 1u to 1am are slice S6b's), and the
// States-index rows of Findings.
import { idsBetween, registerCoverage, registerFrames } from "../search/kit";
import { excluded, frames, states } from "./frames";

registerFrames("findings", [...frames, ...states]);
registerCoverage("findings", "findings", idsBetween("findings", "1a", "1t"), frames, excluded);
