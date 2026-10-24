// Regions of the File View frames that are not compared with the design (a rectangle painted one colour in both pictures before they are
// compared, e2e/visual/access/lib/compare.ts). A mask names the row of design/DEVIATIONS.md that explains the difference, so a mask
// without a reason cannot be written. The other masks of these frames are in view.spec.ts, next to the frames they belong to.
import type { Mask } from "../access/lib/compare";

/** Everything under the top bar: a banner above the title moves it, and the file's body is Monaco and the app's own viewer, not the design's drawing. */
export const CHROME_ONLY: Mask = { x: 224, y: 44, width: 1216, height: 856, why: "the page under the top bar: a banner above the title moves it (the banner's own layout is compared on the frame by a person); File: Diff and large files; File: view" };

/**
 * The header's breadcrumb, from the sidebar's edge side of the top bar to the switcher: the `nav` of both pages is at x 240 to 852.6 and
 * y 12 to 31 (the design's is 19 px high at y 12, the app's 18 at y 12.5), and the mask is that box grown by 3 px for the anti-aliasing of
 * the glyphs at its ends. Nothing else of the top bar is inside it: the switcher starts at x 864.6, and the sidebar ends at x 224.
 *
 * Why it is not compared: on a swimlane's own page the app's breadcrumb is not the design's. A file page's has a crumb for every folder of
 * the path (Swimlanes / sitb / document-service / resources / receipt-logo.jpeg) and the design's frame has the parent folder only
 * (Swimlanes / sitb / resources / receipt-logo.jpeg), so from the third crumb on it is a different text at a different place, and how many of
 * its pixels cross the threshold depends on the glyphs' anti-aliasing of the machine (1,099 of the 1,264 differing pixels of frame 1m on
 * the sandbox's Chromium and 1,253 of 1,423 on the runner's, which draws LCD text) and not on anything the app can change.
 */
export const BREADCRUMB: Mask = { x: 237, y: 9, width: 619, height: 25, why: "Header breadcrumb (design/DEVIATIONS.md: \"Header: switcher and breadcrumb off a swimlane's own pages\"): the app's crumbs are not the design's, so from the third crumb on the text differs" };
