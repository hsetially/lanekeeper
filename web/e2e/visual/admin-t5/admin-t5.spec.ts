// The four Admin sections of T5 (Sentinels, OS Login users, Severity rules, Retention): the frames of frames.ts as app-owned baselines.
import { registerFrames } from "../search/kit";
import { frames } from "./frames";

registerFrames("admin-t5", frames);
