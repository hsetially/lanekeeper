import type { PageProps } from "@/routes/props";
import { DetailFrame } from "./detail/DetailFrame";
import { DriftTab } from "./drift/DriftTab";

export default function DriftPage({ params }: PageProps<"drift">) {
  return (
    <DetailFrame id={params.id} tab="drift" noun="drift">
      {(lane) => <DriftTab lane={lane} />}
    </DetailFrame>
  );
}
