import { createContext, useContext } from "react";

// What the pieces of the chrome ask of each other: the sidebar's toggle, the header's search button and the user menu's
// shortcuts item all act on state that the shell owns. A screen has no use for it: it binds keys with `useShortcut`
// (src/keyboard) and opens nothing of the chrome.

export type ShellUi = {
  readonly sidebarCollapsed: boolean;
  readonly toggleSidebar: () => void;
  readonly openPalette: () => void;
  readonly openShortcuts: () => void;
  /** Load the palette's chunk before it is asked for (on hover or focus of the search button). */
  readonly preloadPalette: () => void;
};

const NOOP_UI: ShellUi = { sidebarCollapsed: false, toggleSidebar: () => undefined, openPalette: () => undefined, openShortcuts: () => undefined, preloadPalette: () => undefined };

export const ShellContext = createContext<ShellUi>(NOOP_UI);
export const useShell = (): ShellUi => useContext(ShellContext);
