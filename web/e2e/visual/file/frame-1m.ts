// File View frame 1m: the binary file, image comparison. It is a module of its own so that the spec of the frame (view.spec.ts) and the
// spec of its masks (breadcrumb-mask.spec.ts) read the same case.
import type { FrameCase } from "../access/lib/matrix";
import { BREADCRUMB, CHROME_ONLY } from "./masks";

export const FRAME_1M: FrameCase = {
  name: "binary file, image comparison",
  board: "Lanekeeper File View.dc.html",
  ids: ["1m"],
  scenario: "design-file-binary",
  path: "/swimlanes/sitb/files?path=document-service%2Fresources%2Freceipt-logo.jpeg&tab=diff",
  masks: [CHROME_ONLY, BREADCRUMB],
};
