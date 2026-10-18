// A box that scrolls and holds nothing the keyboard can reach has to be reachable itself, or a keyboard has no way to scroll it (WCAG
// 2.1.1; axe `scrollable-region-focusable`). That is a focusable element that is not interactive, which `jsx-a11y/no-noninteractive-tabindex`
// is written to stop: for this one case it is the point. Give the box a role and a name too, so that a screen reader says what it is.

/** Spread onto the box: `<div role="region" aria-label="Notes" {...FOCUSABLE_SCROLLER} className="overflow-auto">`. */
export const FOCUSABLE_SCROLLER = { tabIndex: 0 } as const;
