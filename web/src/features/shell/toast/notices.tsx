import { useQueryClient } from "@tanstack/react-query";
import type { Proposal } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { SafeText } from "@/components/app/SafeText";
import { Radio } from "@/icons";
import { proposalKeys } from "@/lib/query/keys";
import { useLiveNotices } from "@/live/LiveProvider";
import { LinkError, linkTo } from "@/routes/links";
import { routeById } from "@/routes/table";
import { proposalQuery } from "@/features/approvals/queries";
import { toasts, type ToastAction, type ToastStore } from "./store";

// What the live layer says a person should be told (src/live/invalidation.ts, `Notice`), as toasts.
//
//  - A scan saw a file change that did not come through this app: always. The event has no author, so the words are the
//    design's "(unknown author)", and the link is "View swimlane" (H-4).
//  - A proposal changed status: only when it is the viewer's own and the new status is a decision. The event carries no
//    author, so the proposal is read once (it is cached after that: an author never changes) and compared with the session's
//    user id. A proposal that cannot be read is not announced: a toast must never be the first place a refusal shows.
//
// The result of a person's OWN write is not a toast (H-3): it is shown in the dialog that made it.

const DECISION_WORDS: Record<string, string> = { approved: "approved", rejected: "rejected", applied: "applied", stale: "marked stale", expired: "expired" };

function swimlaneLink(swimlane: string): ToastAction | undefined {
  try {
    return { label: "View swimlane", href: linkTo("swimlane", { id: swimlane }) };
  } catch (error) {
    if (error instanceof LinkError) return undefined;
    throw error;
  }
}

export function NoticeToasts({ store = toasts }: { store?: ToastStore }) {
  const queryClient = useQueryClient();
  const session = useSession();
  const meId = session.status === "ready" ? session.me.id : null;
  const role = session.status === "ready" ? session.role : null;

  useLiveNotices((notice) => {
    if (notice.kind === "out-of-band-change") {
      const action = swimlaneLink(notice.swimlane);
      store.push({
        key: `oob:${notice.swimlane}:${notice.path}`,
        icon: Radio,
        title: (
          <>
            <SafeText text={notice.swimlane} className="font-mono text-xs" />: <SafeText text={notice.path.split("/").pop() ?? notice.path} className="font-mono text-xs" /> changed on NFS (unknown author)
          </>
        ),
        ...(action ? { action } : {}),
      });
      return;
    }
    const word = DECISION_WORDS[notice.status];
    if (word === undefined || meId === null) return;
    void (async () => {
      let proposal = queryClient.getQueryData<Proposal>(proposalKeys.detail(notice.id));
      if (proposal === undefined) {
        try {
          proposal = await queryClient.query(proposalQuery(notice.id));
        } catch {
          return;
        }
      }
      if (proposal.author !== meId) return;
      const need = routeById("approval").access;
      const canOpen = need === "public" || roleAtLeast(role, need);
      store.push({
        key: `proposal:${notice.id}`,
        kind: notice.status === "approved" || notice.status === "applied" ? "success" : "warning",
        title: `Your proposal #${notice.id} was ${word}`,
        ...(canOpen ? { action: { label: "View approval", href: linkTo("approval", { id: String(notice.id) }) } } : {}),
      });
    })();
  });
  return null;
}
