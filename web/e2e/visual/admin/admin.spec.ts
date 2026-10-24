// Admin: the board frames of "Lanekeeper Admin" (1a to 1bn) and the States-index rows of Admin.
import { idsBetween, registerCoverage, registerFrames } from "../search/kit";
import { excluded, frames, states } from "./frames";

registerFrames("admin", [...frames, ...states]);
registerCoverage("admin", "admin", idsBetween("admin", "1a", "1bn"), frames, excluded);
