import type { PageProps } from "@/routes/props";
import { SwimlaneList } from "./overview/SwimlaneList";

// The swimlane list, the home page. The screen is `overview/SwimlaneList`; this is the route's default export.
export default function SwimlanesPage({ search }: PageProps<"swimlanes">) {
  return <SwimlaneList search={search} />;
}
