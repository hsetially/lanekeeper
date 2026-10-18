// Search (text, settings, docs list): the board frames 1a to 1j and 1n of "Lanekeeper Search and Status.dc.html", and the
// States-index rows of Search.
import { frames, states } from "./frames";
import { registerCoverage, registerFrames } from "./kit";

registerFrames("search", [...frames, ...states]);
registerCoverage("search", "search", ["1a", "1b", "1c", "1d", "1e", "1f", "1g", "1h", "1i", "1j", "1n"], frames, []);
