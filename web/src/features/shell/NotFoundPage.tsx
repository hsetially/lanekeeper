import { NotFoundScreen } from "@/routes/screens";
import type { PageProps } from "@/routes/props";

/** `/404?req=<request id>`: where the hub or a screen sends a person for a path that does not exist. */
export default function NotFoundPage({ search }: PageProps<"not-found">) {
  return <NotFoundScreen {...(search.req ? { requestId: search.req } : {})} />;
}
