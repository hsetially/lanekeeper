import { useQuery } from "@tanstack/react-query";
import { docsForFileQuery, fileFlagsQuery } from "./queries";

/**
 * Whether the file has anything to show in the docs panel: a document that describes it, or a documented flag it sets. The
 * button for the panel appears only when it does. Until the answer is known (or when it failed) there is no button; the panel
 * itself says what went wrong if someone opens it by its address. These are the panel's own queries, so opening it costs nothing more.
 */
export function useFileDocs(lane: string, file: string | null): "none" | "available" {
  const docs = useQuery({ ...docsForFileQuery(file ?? ""), enabled: file !== null });
  const flags = useQuery({ ...fileFlagsQuery(lane, file ?? ""), enabled: file !== null });
  if (file === null) return "none";
  return (docs.data?.hits.length ?? 0) > 0 || (flags.data?.flags.length ?? 0) > 0 ? "available" : "none";
}
