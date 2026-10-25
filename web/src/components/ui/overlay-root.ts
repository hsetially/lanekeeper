import { useLayoutEffect, useState } from "react";

// Where an overlay that is not a dialog (a tooltip, a menu, a popover) puts its content.
//
// Radix portals to <body> by default. That is outside every landmark, and axe's `region` rule (all page content belongs to a
// landmark) names the visually hidden `role="tooltip"` copy that Radix writes for every open tooltip, and the content of a menu or
// a popover. A dialog is exempt from the rule (axe lists `role=dialog` and `role=alertdialog` as regions) and stays under <body>.
//
// The page's one <main> is a landmark every screen has (the shell draws it, and so does a public page). An overlay is positioned
// `fixed`, and <main> has no transform, filter or containment, so it is no more of a containing block than <body> is: the overlay
// is placed and stacked as it was. With no <main> (a component test, a test page) the overlay goes under <body>, as before.
function mainElement(): HTMLElement | undefined {
  if (typeof document === "undefined") return undefined;
  return document.getElementById("main") ?? undefined;
}

/**
 * The container to give a Radix `Portal`. An overlay opens after the page is drawn, so <main> is there at the first render; one
 * that is open in the very first commit (a test) finds it a moment later, in the layout effect.
 */
export function useOverlayContainer(): HTMLElement | undefined {
  const [container, setContainer] = useState<HTMLElement | undefined>(mainElement);
  useLayoutEffect(() => {
    setContainer(mainElement());
  }, []);
  return container;
}
