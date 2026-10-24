// "As served" (T6, slice C): the frames of frames.ts as app-owned baselines, each with a check in words that runs in every run.
import { registerFrames } from "../pickup/runner";
import { frames } from "./frames";

registerFrames("served", frames);
