import type { ReactNode } from "react";
import { EmptyState } from "@/components/app/EmptyState";
import { Hourglass } from "@/icons";
import type { PageProps } from "@/routes/props";
import { AdminFrame } from "./AdminFrame";
import { areaCopy, isAdminArea, type AdminArea } from "./nav";
import { FoldersSection } from "./sections/FoldersSection";
import { McpSection } from "./sections/McpSection";
import { MergeSection } from "./sections/MergeSection";
import { NotifySection } from "./sections/NotifySection";
import { PathsSection } from "./sections/PathsSection";
import { TiersSection } from "./sections/TiersSection";

// The sections of /admin/:section that are not a screen of their own. The route table has already checked that `section` is one of
// the area's sections; each is built in a file of its own. Four (sentinels, OS Login, severity rules and retention) belong to task
// T5 and are listed here as not available until its files are added: a link to one opens its page and says so, never a blank.

function NotYet({ area }: { area: AdminArea }) {
  return (
    <div className="rounded-md border border-border bg-card">
      <EmptyState icon={Hourglass} title="Not available yet">
        {areaCopy(area).title} is not part of this build of the Admin area yet.
      </EmptyState>
    </div>
  );
}

function body(area: AdminArea): ReactNode {
  switch (area) {
    case "paths":
      return <PathsSection />;
    case "merge":
      return <MergeSection />;
    case "tiers":
      return <TiersSection />;
    case "folders":
      return <FoldersSection />;
    case "notify":
      return <NotifySection />;
    case "mcp":
      return <McpSection />;
    default:
      return <NotYet area={area} />;
  }
}

export default function SectionPage({ params }: PageProps<"admin-section">) {
  const section = params.section;
  if (!isAdminArea(section)) {
    return (
      <div className="flex flex-col">
        <h1 className="px-6 pt-4 text-xl font-semibold">Admin</h1>
        <EmptyState title="No such section">There is no section of the Admin area at this address.</EmptyState>
      </div>
    );
  }
  return <AdminFrame area={section}>{body(section)}</AdminFrame>;
}
