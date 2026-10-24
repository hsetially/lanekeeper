import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import type { PageProps } from "@/routes/props";
import { ApprovalsScreen } from "./ApprovalsScreen";
import { tabOfStatus, type Tab } from "./model";
import { proposalQuery } from "./queries";

// One proposal, by its address (`/approvals/:id`): the queue with the proposal's drawer open. A link from a Teams card or the
// audit log lands here, and the proposal need not be on the first page of any list: it is asked for by its id.
//
// The queue behind the drawer is on the tab the proposal belongs to when the page opens, and stays there: a decision moves the
// proposal to another tab, and the queue must not change under the person who just decided.
export default function ApprovalPage({ params }: PageProps<"approval">) {
  const id = Number(params.id);
  const proposal = useQuery(proposalQuery(id));
  const [tab, setTab] = useState<Tab | null>(null);
  const first = proposal.data?.status;
  useEffect(() => {
    if (tab === null && first !== undefined) setTab(tabOfStatus(first));
  }, [tab, first]);
  return <ApprovalsScreen tab={tab ?? "pending"} selectedId={id} />;
}
