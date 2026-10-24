import type { ComponentType } from "react";
import type { PageProps, RouteId } from "./props";

// Which module shows each route's screen. Every one is a dynamic import, so each screen is a chunk of its own that loads when
// the route is first used (route-level code splitting, P10); the initial JavaScript holds the router, the session and the
// shell, and none of the screens.
//
// A screen is a module with a DEFAULT EXPORT that takes `PageProps<"<route id>">`: `{ params, search }`, the path parameters
// the router already checked and the search keys the route's `validateSearch` kept (src/routes/props.ts). A page never reads
// the URL by itself and never renders a <main> (the shell provides it). The table below is checked by the compiler: a page
// whose props are not its route's does not compile.
//
// Every route that shows a screen has a line, and every line has a file under `src/features/<x>/`. The shell slice created
// the files as stubs; each feature slice REPLACES ITS OWN files and never edits this table (it is shared). The routes that
// only redirect (the aliases and the server-issued links) show no screen and have no line.
//
// The router takes this table as a default, so a test can replace one loader (`createAppRouter({ pages })`) to see what a
// chunk that cannot be fetched does.

/** What the router gives every page; a route's own props (`PageProps<Id>`) are narrower, which the strict table below proves. */
export type AppPageProps = { params: Record<string, string>; search: Record<string, string | undefined> };
export type PageLoader = () => Promise<{ default: ComponentType<AppPageProps> }>;
export type PageLoaders = Record<string, PageLoader>;

type StrictLoaders = { [Id in RouteId]?: () => Promise<{ default: ComponentType<PageProps<Id>> }> };

const LOADERS = {
  // Before the session. S1 (access).
  signin: () => import("@/features/access/SignInPage"),
  "access-pending": () => import("@/features/access/PendingPage"),
  "access-disabled": () => import("@/features/access/DisabledPage"),
  // Account. S1.
  onboarding: () => import("@/features/access/OnboardingPage"),
  "github-token": () => import("@/features/access/GithubTokenPage"),
  "mcp-tokens": () => import("@/features/access/McpTokensPage"),
  // Swimlanes. S2 owns the overview and every tab of a swimlane except Files and the adoption wizard.
  swimlanes: () => import("@/features/swimlanes/SwimlanesPage"),
  swimlane: () => import("@/features/swimlanes/SwimlaneDetailPage"),
  services: () => import("@/features/swimlanes/ServicesPage"),
  drift: () => import("@/features/swimlanes/DriftPage"),
  activity: () => import("@/features/swimlanes/ActivityPage"),
  "swimlane-findings": () => import("@/features/swimlanes/SwimlaneFindingsPage"),
  "swimlane-audit": () => import("@/features/swimlanes/SwimlaneAuditPage"),
  "swimlane-pull-requests": () => import("@/features/prs/PullRequestsPage"),
  "swimlane-served": () => import("@/features/served/ServedPage"),
  // File view and the draft screen. S3.
  files: () => import("@/features/file/FilePage"),
  draft: () => import("@/features/file/DraftPage"),
  // Across swimlanes. S4 compare; S5 search, docs and feature status; S6a findings; S6b adoption; S7 approvals and audit.
  compare: () => import("@/features/compare/ComparePage"),
  status: () => import("@/features/status/StatusPage"),
  search: () => import("@/features/search/SearchPage"),
  doc: () => import("@/features/docs/DocPage"),
  "doc-view": () => import("@/features/docs/DocViewPage"),
  findings: () => import("@/features/findings/FindingsPage"),
  adopt: () => import("@/features/adoption/AdoptPage"),
  approvals: () => import("@/features/approvals/ApprovalsPage"),
  approval: () => import("@/features/approvals/ApprovalPage"),
  audit: () => import("@/features/audit/AuditPage"),
  // Admin. S8.
  "admin-users": () => import("@/features/admin/UsersPage"),
  "admin-swimlanes": () => import("@/features/admin/SwimlanesAdminPage"),
  "admin-section": () => import("@/features/admin/SectionPage"),
  // Errors. S0 (the shell) owns these three.
  "not-found": () => import("@/features/shell/NotFoundPage"),
  error: () => import("@/features/shell/ErrorPage"),
  forbidden: () => import("@/features/shell/ForbiddenPage"),
} satisfies StrictLoaders;

// `satisfies StrictLoaders` above proves that each page takes exactly the props its own route gives it, and the router renders a
// page with the props of the route it matched. TypeScript cannot relate "this id has these props" across a record, so the one
// widening to the loose type is this cast.
export const PAGE_LOADERS = LOADERS as unknown as PageLoaders;
