import { Navigate } from "@tanstack/react-router";
import { Role } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { RouteLoading } from "./screens";

/**
 * `/`: an Editor or above whose GitHub token is missing goes to onboarding, everyone else to the swimlanes. Only the
 * landing page leads anywhere: the rest of the app stays readable, and a Viewer never sees the onboarding page.
 */
export function Landing() {
  const session = useSession();
  if (session.status !== "ready") return <RouteLoading />;
  const needsToken = session.me.needs_github_token && roleAtLeast(session.role, Role.editor);
  return <Navigate to={needsToken ? "/access/onboarding" : "/swimlanes"} replace />;
}
