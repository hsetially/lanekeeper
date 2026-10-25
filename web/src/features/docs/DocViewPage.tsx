import { EmptyState } from "@/components/app/EmptyState";
import { BookOpen } from "@/icons";
import type { PageProps } from "@/routes/props";
import { AppLink } from "@/routes/AppLink";
import { DocReader } from "./DocReader";
import { isDocPath } from "./doc-path";

/** What the reader shows for an address that is not a doc path. The server is not asked. */
export function NotADoc() {
  return (
    <EmptyState
      icon={BookOpen}
      as="h1"
      title="That address is not a doc"
      className="pt-16"
      action={
        <AppLink route="search" search={{ type: "docs" }} className="text-sm underline">
          Back to Docs
        </AppLink>
      }
    >
      A doc is opened from a search result or from Docs, by its path.
    </EmptyState>
  );
}

// `/docs/view?path=`: the address that the contract gives for a doc (`DocHit.anchor` says the reader opens
// `/docs/view?path=<path>#<anchor>`) and the links in the MCP results.
export default function DocViewPage({ search }: PageProps<"doc-view">) {
  const path = search.path;
  if (path === undefined || !isDocPath(path)) return <NotADoc />;
  return <DocReader path={path} />;
}
