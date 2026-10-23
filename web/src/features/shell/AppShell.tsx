import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useRouterState } from "@tanstack/react-router";
import { useSession } from "@/auth/session";
import { NewVersionBanner } from "@/components/app/NewVersion";
import { PausedBanner } from "@/components/app/PausedBanner";
import { KeyboardProvider, useShortcut } from "@/keyboard";
import { prefs } from "@/lib/prefs";
import { hrefFor, linkTo } from "@/routes/links";
import { useNavigateTo } from "@/routes/AppLink";
import type { RouteId } from "@/routes/props";
import { chooseSwimlane } from "./current-swimlane";
import { useDocumentTitle, useFocusHeadingOnNavigation } from "./focus";
import { Header, HeaderSkeleton } from "./Header";
import { mayOpen } from "./nav";
import type { PaletteAction } from "./palette/model";
import { ShellContext, type ShellUi } from "./ShellContext";
import { Sidebar, SidebarSkeleton } from "./Sidebar";
import { THEME_CHOICES, useTheme } from "./theme";
import { NoticeToasts } from "./toast/notices";
import { Toaster } from "./toast/Toaster";

// The app shell (the Shell, frame 1a): the sidebar, the header, the live banner and the content, around whatever the router
// puts in the content (`children`, the Outlet). It owns what the chrome shares: whether the sidebar is collapsed, whether the
// palette and the shortcuts sheet are open, the keys that open them, and the toaster.
//
//  - A PUBLIC route (sign-in, the pending and disabled pages, the error screens) has no chrome: there is no one to show a role
//    for, and no shortcut runs there (the keyboard registry is not mounted).
//  - While the session loads, the frame is drawn without items, so the page does not jump when the role arrives.
//  - The shell provides the <main> landmark. A screen renders into it and never renders a <main> of its own.
//  - The palette and the shortcuts sheet are chunks of their own, loaded when idle and when first asked for.

const Palette = lazy(() => import("./palette/CommandPalette"));
const Sheet = lazy(() => import("./ShortcutsSheet"));
const loadPalette = (): void => {
  void import("./palette/CommandPalette");
};

/** The screens that the Go to keys open, in the order of the binding: g s, g c, g f, g i, g a. */
const GO_TO: readonly RouteId[] = ["swimlanes", "compare", "status", "findings", "audit"];

function saveDataOn(): boolean {
  const connection = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection;
  return connection?.saveData === true;
}

/** Load the palette's chunk when the browser is idle, so that the first ⌘K opens it at once. */
function useIdlePalette(enabled: boolean): void {
  useEffect(() => {
    if (!enabled || saveDataOn()) return undefined;
    if (typeof requestIdleCallback === "function") {
      const handle = requestIdleCallback(loadPalette, { timeout: 6000 });
      return () => {
        if (typeof cancelIdleCallback === "function") cancelIdleCallback(handle);
      };
    }
    const timer = setTimeout(loadPalette, 1500);
    return () => {
      clearTimeout(timer);
    };
  }, [enabled]);
}

/** The frame itself: skip link, the sidebar column and the main column. `main` is the one landmark of the screen. */
function Frame({ sidebar, header, children, mainRef }: { sidebar: ReactNode; header: ReactNode; children: ReactNode; mainRef: React.RefObject<HTMLElement | null> }) {
  return (
    <div className="flex h-dvh w-full overflow-hidden bg-background text-foreground">
      <a
        href="#main"
        onClick={(event) => {
          event.preventDefault();
          mainRef.current?.focus();
        }}
        className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-[60] focus:rounded-sm focus:border focus:border-border focus:bg-popover focus:px-3 focus:py-1.5 focus:shadow-popover"
      >
        Skip to content
      </a>
      {sidebar}
      <div className="flex min-w-0 flex-1 flex-col">
        {header}
        <PausedBanner />
        <NewVersionBanner />
        <main id="main" ref={mainRef} tabIndex={-1} className="flex min-h-0 flex-1 flex-col overflow-auto outline-none">
          {/* The screen's box is a stacking context, so a layer a screen draws (a sticky table header, z-10) stays under a dialog. <main> is not one:
              the menus, popovers and tooltips are portaled into it and must stack above the sidebar (ui/overlay-root.ts). */}
          <div className="isolate mx-auto flex min-h-0 w-full max-w-[1680px] flex-1 flex-col">{children}</div>
        </main>
      </div>
    </div>
  );
}

/** What the chrome does once the person is known: the keys, the overlays, the toaster. */
function Chrome({ children, mainRef }: { children: ReactNode; mainRef: React.RefObject<HTMLElement | null> }) {
  const session = useSession();
  const navigate = useNavigateTo();
  const theme = useTheme();
  const me = session.status === "ready" ? session.me : null;
  const role = session.status === "ready" ? session.role : null;

  const [collapsed, setCollapsed] = useState(() => prefs.get("sidebarCollapsed"));
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteWanted, setPaletteWanted] = useState(false);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [sheetWanted, setSheetWanted] = useState(false);

  const collapsedRef = useRef(collapsed);
  const toggleSidebar = useCallback(() => {
    const next = !collapsedRef.current;
    collapsedRef.current = next;
    prefs.set("sidebarCollapsed", next);
    setCollapsed(next);
  }, []);
  const openPalette = useCallback(() => {
    setPaletteWanted(true);
    setSheetOpen(false);
    setPaletteOpen(true);
  }, []);
  const openShortcuts = useCallback(() => {
    setSheetWanted(true);
    setPaletteOpen(false);
    setSheetOpen(true);
  }, []);

  useIdlePalette(session.status === "ready" && role !== null);

  useShortcut("global.command-palette", () => {
    openPalette();
  });
  useShortcut("global.shortcuts-sheet", () => {
    openShortcuts();
  });
  useShortcut("global.toggle-sidebar", () => {
    toggleSidebar();
  });
  // `/` outside the search page goes there; on the search page its own row (layer 1) takes the key first.
  useShortcut("global.search-text-focus-query", () => {
    navigate(hrefFor("search", {}, { type: "text" }));
  });
  useShortcut("go-to.swimlanes-compare-feature-status-findings", (hit) => {
    const route = GO_TO[hit.binding];
    // A key to a screen the role cannot open does nothing, instead of landing on the permission screen.
    if (route === undefined || !mayOpen(role, route)) return false;
    navigate(linkTo(route as "swimlanes"));
    return undefined;
  });

  const run = useCallback(
    (action: PaletteAction): void => {
      if (action.kind === "go") {
        if (action.swimlane !== undefined) chooseSwimlane(action.swimlane);
        navigate(action.href);
        return;
      }
      switch (action.what) {
        case "shortcuts":
          openShortcuts();
          break;
        case "sidebar":
          toggleSidebar();
          break;
        default: {
          const wanted = THEME_CHOICES.find((c) => `theme-${c.value}` === action.what);
          if (wanted) theme.set(wanted.value);
        }
      }
    },
    [navigate, openShortcuts, toggleSidebar, theme],
  );

  const ui = useMemo<ShellUi>(() => ({ sidebarCollapsed: collapsed, toggleSidebar, openPalette, openShortcuts, preloadPalette: loadPalette }), [collapsed, toggleSidebar, openPalette, openShortcuts]);

  return (
    <ShellContext.Provider value={ui}>
      <Frame sidebar={me ? <Sidebar me={me} role={role} /> : <SidebarSkeleton />} header={me ? <Header /> : <HeaderSkeleton />} mainRef={mainRef}>
        {children}
      </Frame>
      {paletteWanted ? (
        <Suspense fallback={null}>
          <Palette open={paletteOpen} onOpenChange={setPaletteOpen} role={role} onRun={run} />
        </Suspense>
      ) : null}
      {sheetWanted ? (
        <Suspense fallback={null}>
          <Sheet open={sheetOpen} onOpenChange={setSheetOpen} role={role} />
        </Suspense>
      ) : null}
      <NoticeToasts />
      <Toaster />
    </ShellContext.Provider>
  );
}

export function AppShell({ children }: { children: ReactNode }) {
  const session = useSession();
  const access = useRouterState({ select: (s) => s.matches.at(-1)?.staticData.access });
  const failed = useRouterState({ select: (s) => ["notFound", "error"].includes(s.matches.at(-1)?.status ?? "") });
  const mainRef = useRef<HTMLElement | null>(null);
  useFocusHeadingOnNavigation(mainRef);
  useDocumentTitle();

  // A route that did not resolve (a path parameter the table refused, an error) is shown plain, as an unknown path is.
  const signedInRoute = access !== undefined && access !== "public" && !failed;
  // A session that failed has no one to draw chrome for: the gate shows the error, in a plain frame.
  const chrome = signedInRoute && (session.status === "ready" || session.status === "loading" || session.status === "none");
  if (!chrome) {
    return (
      <main id="main" ref={mainRef} tabIndex={-1} className="flex min-h-dvh flex-col outline-none">
        <NewVersionBanner />
        {children}
      </main>
    );
  }
  return (
    <KeyboardProvider>
      <Chrome mainRef={mainRef}>{children}</Chrome>
    </KeyboardProvider>
  );
}
