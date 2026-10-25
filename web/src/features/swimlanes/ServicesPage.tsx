import type { PageProps } from "@/routes/props";
import { DetailFrame } from "./detail/DetailFrame";
import { ServicesTab } from "./services/ServicesTab";

export default function ServicesPage({ params, search }: PageProps<"services">) {
  return (
    <DetailFrame id={params.id} tab="services" noun="services">
      {(lane) => <ServicesTab lane={lane} q={search.q ?? ""} />}
    </DetailFrame>
  );
}
