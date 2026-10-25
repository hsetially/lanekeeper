// The pull-request pieces (T5, slice B): the frames of frames.ts as app-owned baselines.
import { registerFrames } from "../search/kit";
import { frames } from "./frames";

registerFrames("prs", frames);
