// The pickup chips (T6, slice A): the frames of frames.ts as app-owned baselines, each with a check in words that runs in every run.
import { frames } from "./frames";
import { registerFrames } from "./runner";

registerFrames("pickup", frames);
