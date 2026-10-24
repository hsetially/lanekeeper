// Notify services, auto-notify, the config-server restart and restart all (T6, slice B): the frames of frames.ts as app-owned baselines, each
// with a check in words that runs in every run.
import { registerFrames } from "../pickup/runner";
import { frames } from "./frames";

registerFrames("notify", frames);
