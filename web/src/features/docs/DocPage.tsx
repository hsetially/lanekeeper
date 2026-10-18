import type { PageProps } from "@/routes/props";
import { DocReader } from "./DocReader";
import { docPathFromSlug } from "./doc-path";
import { NotADoc } from "./DocViewPage";

// `/docs/:slug`, the handoff's address for a doc. The slug is the doc's path, encoded (src/features/docs/doc-path.ts); a slug in
// plain words does not name a doc, because the same words can be the file of several branches.
export default function DocPage({ params }: PageProps<"doc">) {
  const path = docPathFromSlug(params.slug);
  if (path === undefined) return <NotADoc />;
  return <DocReader path={path} />;
}
