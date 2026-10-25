import type { PageProps } from "@/routes/props";
import { DetailFrame } from "./detail/DetailFrame";
import { ServicesTab } from "./services/ServicesTab";

// A swimlane, as its overview: the Services tab is the tab that opens (the design's default). The other tabs are their own routes.
export default function SwimlaneDetailPage({ params }: PageProps<"swimlane">) {
  return (
    <DetailFrame id={params.id} tab="services" noun="services">
      {(lane) => <ServicesTab lane={lane} q="" />}
    </DetailFrame>
  );
}
