// The doc reader and the upload, replace and delete dialogs: board frames 1k to 1m and 1o to 1t of "Lanekeeper Search and Status".
import { registerCoverage, registerFrames } from "../search/kit";
import { frames, states } from "./frames";

registerFrames("docs", [...frames, ...states]);
registerCoverage("docs", "search", ["1k", "1l", "1m", "1o", "1p", "1q", "1r", "1s", "1t"], frames, []);
