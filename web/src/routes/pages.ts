import type { ComponentType } from "react";

// Which module shows each route's screen. Every one is a dynamic import, so each screen is a chunk of its own that loads
// when the route is first used (route-level code splitting, P10); the initial JavaScript holds the router, the session and
// the shell, and none of the screens.
//
// A screen is a module with a default export that takes no props: it reads its search parameters with `useSearch`, as the
// route's `validateSearch` shaped them. The shell slice and the feature slices (T4) replace a line here with the import of
// their page, `() => import("@/features/<x>/<X>Page")`; nothing else in the router changes. The routes that only redirect
// (the aliases and the server-issued links) show no screen and have no line.
//
// The router takes this table as a default, so a test can replace one loader (`createAppRouter({ pages })`) to see what a
// chunk that cannot be fetched does.

export type PageLoader = () => Promise<{ default: ComponentType }>;
export type PageLoaders = Record<string, PageLoader>;

/** What every route shows until its screen is built. One module, so one chunk for all of them. */
const placeholder: PageLoader = () => import("./Placeholder");

export const PAGE_LOADERS: PageLoaders = {
  // Before the session.
  signin: () => import("./SignIn").then((module) => ({ default: module.SignIn })),
  "access-pending": placeholder,
  "access-disabled": placeholder,
  // Account.
  onboarding: placeholder,
  "github-token": placeholder,
  "mcp-tokens": placeholder,
  // Swimlanes.
  swimlanes: placeholder,
  swimlane: placeholder,
  files: placeholder,
  services: placeholder,
  "swimlane-findings": placeholder,
  "swimlane-audit": placeholder,
  adopt: placeholder,
  drift: placeholder,
  activity: placeholder,
  // Across swimlanes.
  compare: placeholder,
  status: placeholder,
  search: placeholder,
  doc: placeholder,
  "doc-view": placeholder,
  findings: placeholder,
  approvals: placeholder,
  approval: placeholder,
  draft: placeholder,
  audit: placeholder,
  // Admin.
  "admin-users": placeholder,
  "admin-swimlanes": placeholder,
  "admin-section": placeholder,
  // Errors.
  "not-found": placeholder,
  error: placeholder,
  forbidden: placeholder,
};
