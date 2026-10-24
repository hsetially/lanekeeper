// Feature status: board frames 1u to 1x of "Lanekeeper Search and Status", and the States-index rows of Feature status.
import { registerCoverage, registerFrames } from "../search/kit";
import { frames, states } from "./frames";

registerFrames("status", [...frames, ...states]);
registerCoverage("status", "search", ["1u", "1v", "1w", "1x"], frames, []);
