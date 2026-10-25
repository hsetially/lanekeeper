import type { PageProps } from "@/routes/props";
import { ApprovalsScreen } from "./ApprovalsScreen";
import { tabOfSearch } from "./model";

// The approval queue (Admin only; the route gate shows everyone else the permission screen). The screen is `ApprovalsScreen`;
// this is the route's default export. The tab is in the address (`?state=`), so a link keeps it.
export default function ApprovalsPage({ search }: PageProps<"approvals">) {
  return <ApprovalsScreen tab={tabOfSearch(search.state)} selectedId={null} />;
}
