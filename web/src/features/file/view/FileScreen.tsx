import { useQuery } from "@tanstack/react-query";
import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import type { ValidationIssue } from "@/api/generated/model";
import { roleAtLeast } from "@/auth/roles";
import { useSession } from "@/auth/session";
import { Banner } from "@/components/app/Banner";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { useWriteGate } from "@/components/app/write-gate";
import { isContentWithheld, withheldReason } from "@/components/masking/withheld";
import { PrBadge } from "@/components/pr/PrBadge";
import { useEditorWarmup } from "@/editor/warm";
import { useShortcut } from "@/keyboard";
import { useQueryState } from "@/lib/query/use-query-state";
import { useNow } from "@/live/use-now";
import { useNavigateTo } from "@/routes/AppLink";
import { linkTo } from "@/routes/links";
import { FileChangedBy, useActorNames } from "@/features/attribution";
import { FilePickupActions } from "@/features/notify/FilePickupActions";
import { FilePickup } from "@/features/pickup";
import { usePrBadges } from "@/features/prs/use-pr-badges";
import { swimlaneQuery } from "@/features/swimlanes/queries";
import { DiffPane } from "../diff/DiffPane";
import { diffParamOf, type Side } from "../diff/refs";
import { useComparison } from "../diff/useComparison";
import { EditPane } from "../edit/EditPane";
import { issueMessage, type WriteOutcome } from "../edit/outcome";
import { useEditSession } from "../edit/useEditSession";
import { DocsPanel } from "../docs-panel/DocsPanel";
import { useFileDocs } from "../docs-panel/useFileDocs";
import { HistoryPanel } from "../history/HistoryPanel";
import { effectiveStem, fileFacts, LARGE_FILE_LINES, lineCount, splitPath, type Panel, type Tab } from "../model";
import { fileHeadQuery } from "../queries";
import { AgentOfflineBanner, ChangedWhileEditingBanner, EditingStrip, LargeFileBanner, ReadOnlyBanner, WithheldBanner } from "./Banners";
import { ContentView } from "./ContentView";
import { FileActions } from "./FileActions";
import { FileHeader } from "./FileHeader";
import { FileTabs } from "./FileTabs";
import { fileGates } from "./gates";
import { RefTab } from "./RefTab";
import type { Dialog } from "./FileDialogs";
import type { Settled, WriteSource } from "./SettledDialogs";
import { useFileEntry } from "./useFileEntry";

// The file screen: the head, the tabs and the panels around them, the edit session and every dialog of a write. It decides what
// is shown from what the server answered, and writes nothing itself: each write is a dialog that carries the hash the file had
// when the person saw it, and its answer (applied, sent for approval, a conflict, a lock) is shown, never assumed (W17).

export type FileScreenProps = {
  id: string;
  path: string;
  tab: Tab;
  panel: Panel | null;
  side: Side;
  line: number | null;
};

type Notice = { source: WriteSource; warnings: readonly ValidationIssue[] };

// Every dialog is one chunk (FileDialogs.tsx): not on the way to a readable file. It is loaded when the browser is idle after the file has come, or
// at once when the first dialog is asked for, and stays mounted from then on so that a dialog can close the way it opened.
const loadDialogs = () => import("./FileDialogs");
const FileDialogs = lazy(loadDialogs);

const NOTICE_TITLE: Record<WriteSource, string> = {
  save: "Saved",
  convert: "Line endings converted",
  revert: "Reverted",
  upload: "Uploaded",
  delete: "Deleted",
  pr: "Pull request raised",
};

export function FileScreen({ id, path, tab, panel, side, line }: FileScreenProps) {
  const session = useSession();
  const me = session.status === "ready" ? session.me : null;
  const role = session.status === "ready" ? session.role : null;
  const goTo = useNavigateTo();
  const base = useWriteGate("editor");

  const detail = useQuery(swimlaneQuery(id)).data;
  const entryState = useFileEntry(id, path);
  const entry = entryState.status === "found" ? entryState.entry : null;
  const headQuery = useQuery(fileHeadQuery(id, path));
  const head = headQuery.data;
  const headState = useQueryState(headQuery);
  // One read of the swimlane's pull-request links for the whole screen (the header's badge), and one of the user names for an Admin.
  const prs = usePrBadges(id);
  const { nameOf } = useActorNames();

  const edit = useEditSession();
  const editing = edit.session !== null;
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [settled, setSettled] = useState<Settled | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [issues, setIssues] = useState<readonly ValidationIssue[]>([]);
  const [tokenExpired, setTokenExpired] = useState(false);
  const [dialogsWanted, setDialogsWanted] = useState(false);
  const [layout, setLayout] = useState<"split" | "unified">("split");
  const [swapped, setSwapped] = useState(false);
  const now = useNow(detail !== undefined && detail.agent !== "connected", 30_000);

  const { name } = splitPath(path);
  const logical = entry?.logical_file ?? null;
  const tenant = entry?.tenant ?? null;
  const title = logical === null ? name : splitPath(logical).name;
  const withheld = head !== undefined && isContentWithheld(head);
  const masked = (head?.masks?.length ?? 0) > 0;
  const lines = useMemo(() => (head?.text == null ? 0 : lineCount(head.text)), [head?.text]);
  const large = lines > LARGE_FILE_LINES;
  const gates = fileGates({
    base,
    agent: detail?.agent,
    lane: id,
    masked,
    fileClass: head?.class,
    withheld,
    tokenExpired,
    needsGithubToken: me?.needs_github_token ?? false,
  });
  const docs = useFileDocs(id, logical);
  const canText = head !== undefined && !withheld && head.class !== "binary";

  // The Diff tab's count follows the side that is picked; the note under the name ("Line endings only") is about Git, whatever is picked.
  const picked = useComparison({
    lane: id,
    path,
    entry,
    detail,
    side,
    swapped,
    enabled: canText && !editing,
  });
  const againstGit = useComparison({
    lane: id,
    path,
    entry,
    detail,
    side: { kind: "git" },
    swapped: false,
    enabled: canText && !editing,
  });
  const diffCount = picked.query.data?.setting_changes.length ?? null;
  const gitLabel = picked.context === null ? "" : picked.context.tenant !== null ? `(${picked.context.tenant} head)` : `(${picked.context.baseLabel})`;

  useEditorWarmup(head !== undefined);

  // The dialogs' chunk: asked for by the first dialog or answer, or fetched when the browser is idle once the file is there.
  if (!dialogsWanted && (dialog !== null || settled !== null)) setDialogsWanted(true);
  const fileHere = head !== undefined;
  useEffect(() => {
    if (!fileHere) return undefined;
    const warm = (): void => {
      void loadDialogs().catch(() => undefined);
    };
    if (typeof requestIdleCallback === "function") {
      const handle = requestIdleCallback(warm, { timeout: 6000 });
      return () => {
        if (typeof cancelIdleCallback === "function") cancelIdleCallback(handle);
      };
    }
    const timer = setTimeout(warm, 1500);
    return () => {
      clearTimeout(timer);
    };
  }, [fileHere]);

  const href = (over: { tab?: Tab; panel?: Panel | null; side?: Side } = {}): string => {
    const nextTab = over.tab ?? tab;
    const nextPanel = over.panel === undefined ? panel : over.panel;
    const nextSide = over.side ?? side;
    return linkTo(
      "files",
      { id },
      {
        path,
        ...(nextTab === "nfs" ? {} : { tab: nextTab }),
        ...(nextPanel === null ? {} : { panel: nextPanel }),
        ...(nextSide.kind === "git" ? {} : { diff: diffParamOf(nextSide) }),
        ...(over.tab === undefined && line !== null ? { line: String(line) } : {}),
      },
    );
  };

  const startEdit = (): void => {
    if (head?.text == null || !gates.edit.allowed) return;
    edit.start({ hash: head.hash, text: head.text });
    setIssues([]);
    setNotice(null);
  };
  const cancelEdit = (): void => {
    if (edit.isDirty()) setDialog({ kind: "discard" });
    else {
      edit.end();
      setIssues([]);
    }
  };
  const openSave = (): void => {
    if (edit.isDirty() && gates.upload.allowed) setDialog({ kind: "save" });
  };
  const discard = (): void => {
    edit.end();
    setIssues([]);
    setSettled(null);
    setDialog(null);
    void headQuery.refetch();
  };

  useShortcut("file-viewer.edit", startEdit, {
    enabled: !editing && head?.text != null && gates.edit.allowed,
  });
  useShortcut("file-viewer.review-and-save-opens-the", openSave, {
    enabled: editing,
    allow: { editor: true, input: true },
  });
  useShortcut("file-viewer.cancel-edit-asks-if-there", cancelEdit, {
    enabled: editing,
    allow: { editor: true },
  });

  const settle = (source: WriteSource, outcome: WriteOutcome): void => {
    switch (outcome.kind) {
      case "applied":
        if (source === "save" || source === "convert") edit.end();
        setIssues([]);
        if (source === "delete") {
          goTo(linkTo("swimlane", { id }));
          return;
        }
        setNotice({ source, warnings: outcome.warnings });
        return;
      case "sent":
        if (source === "save") edit.end();
        setSettled({ kind: "sent", proposal: outcome.proposal, source });
        return;
      case "conflict":
        // The file is not what the screen has: read it again, so that what the screen says next is the server's.
        void headQuery.refetch();
        setSettled({
          kind: "conflict",
          current: outcome.current,
          requestId: outcome.requestId,
          source,
        });
        return;
      case "locked":
        setSettled({
          kind: "locked",
          until: outcome.until,
          requestId: outcome.requestId,
          source,
        });
        return;
      default:
        // A refused or failed write stays in the dialog that made it, with its request id; nothing reaches here.
        return;
    }
  };

  // The hash the write was based on. A file that was not there (null) is a base of its own: it must not fall back to the head.
  const yourBase = edit.session !== null ? edit.session.base.hash : (head?.hash ?? null);
  const changedUnder = editing && head !== undefined && edit.session !== null && edit.session.base.hash !== head.hash;
  const pickup = entry?.pickup_state;
  const needsPickup = pickup === "needs_notify_or_restart" || pickup === "needs_config_server_restart";
  const docsState = panel === "docs" ? "open" : docs === "available" ? "available" : "none";

  const actions =
    head === undefined ? null : (
      <FileActions
        gates={gates}
        editing={editing}
        withheld={withheld}
        historyOpen={panel === "history"}
        docs={docsState}
        onEdit={startEdit}
        onPr={() => setDialog({ kind: "pr" })}
        onUpload={() => setDialog({ kind: "upload" })}
        onDelete={() => setDialog({ kind: "delete" })}
        onHistory={() => goTo(href({ panel: panel === "history" ? null : "history" }))}
        onDocs={() => goTo(href({ panel: panel === "docs" ? null : "docs" }))}
        asServed={{ id, file: path, tenant }}
      />
    );

  let main: React.ReactNode;
  if (editing && edit.session !== null && head !== undefined) {
    main = (
      <EditPane
        name={name}
        lane={id}
        initialText={edit.session.base.text}
        text={edit.session.text}
        format={head.format}
        dirty={edit.dirty}
        issues={issues}
        approval={me?.requires_approval ?? false}
        saveGate={gates.upload}
        rebasedFrom={edit.session.rebasedFrom}
        onText={(text) => {
          edit.setText(text);
          if (issues.length > 0) setIssues([]);
        }}
        onCancel={cancelEdit}
        onSave={openSave}
      />
    );
  } else if (headState.state === "error" && headState.error?.status === 404) {
    main = (
      <EmptyState title="File not found" action={undefined}>
        {`There is no ${path} in ${id}. It may have been deleted or renamed.`}
      </EmptyState>
    );
  } else if (head === undefined) {
    main = (
      <QueryBoundaryView state={headState} noun="file" source="the file from NFS" className="flex min-h-0 flex-1 flex-col">
        {() => null}
      </QueryBoundaryView>
    );
  } else {
    const compareWithGit = (): void => goTo(href({ tab: "diff" }));
    main = (
      <FileTabs tab={tab} onTab={(next) => goTo(href({ tab: next }))} gitLabel={gitLabel} diffCount={canText && diffCount !== null && diffCount > 0 ? diffCount : null}>
        {withheld || tab === "nfs" ? (
          <ContentView content={head} name={name} where="NFS" highlightLine={line} entry={entry} onCompareWithGit={withheld ? null : compareWithGit} lane={id} />
        ) : tab === "diff" ? (
          <DiffPane
            lane={id}
            path={path}
            name={name}
            entry={entry}
            detail={detail}
            head={head}
            side={side}
            swapped={swapped}
            layout={layout}
            onSide={(next) => goTo(href({ side: next }))}
            onSwap={() => setSwapped((value) => !value)}
            onLayout={setLayout}
            convertGate={gates.edit}
            onConvert={(to) => setDialog({ kind: "convert", to })}
          />
        ) : (
          <RefTab source={tab} lane={id} path={path} name={name} entry={entry} entryKnown={entryState.status !== "loading"} detail={detail} line={line} onCompare={compareWithGit} />
        )}
      </FileTabs>
    );
  }

  // What a write that was applied tells the person: the file is the new version, who still has to pick it up, and the warnings (a
  // key written twice is allowed, and said).
  const noticeText =
    notice === null
      ? ""
      : [notice.source === "pr" ? "" : `The file on ${id} NFS is the new version.`, needsPickup && notice.source !== "pr" ? "Services pick it up after a notify or a restart." : "", ...notice.warnings.map(issueMessage)]
          .filter((part) => part !== "")
          .join(" ");
  const close = (): void => setDialog(null);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {notice === null ? null : (
        <Banner
          variant="success"
          title={`${NOTICE_TITLE[notice.source]}${notice.source === "save" ? ` to ${id} NFS` : ""}`}
          onDismiss={() => setNotice(null)}
          action={detail !== undefined && notice.source !== "pr" ? <FilePickupActions lane={detail} path={path} pickup={entry?.pickup_state} /> : undefined}
        >
          {noticeText}
        </Banner>
      )}
      {role !== null && !roleAtLeast(role, "editor") ? <ReadOnlyBanner /> : null}
      {detail !== undefined && detail.agent !== "connected" ? <AgentOfflineBanner lane={id} asOf={detail.last_scan_at} now={now} canCheck={roleAtLeast(role, "admin")} /> : null}
      <FileHeader
        title={title}
        marker={tenant !== null && logical !== null ? `Effective file for ${effectiveStem(logical)}` : null}
        drift={entry?.drift ?? null}
        note={againstGit.query.data?.eol_only === true ? "Line endings only" : null}
        editing={editing}
        path={path}
        facts={head === undefined ? null : fileFacts(head, name)}
        hash={head?.hash ?? null}
        unavailable={headState.state === "error"}
        actions={actions}
        link={linkTo("files", { id }, { path, ...(line === null ? {} : { line: String(line) }) })}
        linkLine={line}
        prBadge={<PrBadge info={prs.badgeFor({ path, hash: head?.hash ?? entry?.hash ?? null, drift: entry?.drift ?? null })} />}
        pickup={<FilePickup entry={entry} />}
        // The chip is 24 px high (its target size) and the facts beside it are 22: a 1 px margin above and below keeps the row 22 px tall.
        changedBy={entry === null ? null : <FileChangedBy attribution={entry.attribution} nameOf={nameOf} className="-my-px" />}
        copyHash={!withheld}
      />
      {/* Under the header, where the handoff has them (frame 3 of File). The file brings them, and the header stands on the page as a
          placeholder before that, so when they come the header does not move. */}
      {withheld ? <WithheldBanner reason={withheldReason(head) ?? "denied"} /> : null}
      {head !== undefined && large && !editing && canText ? <LargeFileBanner lines={lines} gate={gates.edit} onEnableEditing={startEdit} /> : null}
      {editing ? <EditingStrip lane={id} file={name} /> : null}
      {changedUnder ? <ChangedWhileEditingBanner /> : null}
      {/* The body is the placeholder ("Loading file…") and then the file: two trees, with a key that changes once, when the file comes. What
          the file brings above it (a banner) does not count as the placeholder having moved (Cumulative Layout Shift, P10), and nothing the
          person was reading is replaced: the file stays, refreshing, from then on. */}
      <div key={head === undefined ? "placeholder" : "file"} className="flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 flex-col">{main}</div>
        {panel === "history" ? (
          <HistoryPanel id={id} path={path} currentHash={head?.hash ?? null} revertGate={gates.revert} onRevert={(version) => setDialog({ kind: "revert", version })} onClose={() => goTo(href({ panel: null }))} copyHash={!withheld} />
        ) : panel === "docs" ? (
          <DocsPanel lane={id} file={entryState.status === "loading" ? null : (logical ?? path)} onClose={() => goTo(href({ panel: null }))} />
        ) : null}
      </div>

      {dialogsWanted ? (
        <Suspense fallback={null}>
          <FileDialogs
            head={head}
            lane={id}
            path={path}
            name={name}
            dialog={dialog}
            settled={settled}
            session={edit.session}
            entry={entry}
            detail={detail}
            requiresApproval={me?.requires_approval ?? false}
            yourBase={yourBase}
            canAdopt={roleAtLeast(role, "admin")}
            canOpenApprovals={roleAtLeast(role, "admin")}
            onClose={close}
            onSettled={settle}
            onIssues={setIssues}
            onTokenExpired={() => setTokenExpired(true)}
            onDiscard={discard}
            onViewCurrent={() => setDialog({ kind: "versions" })}
            onReapply={() => {
              if (settled?.kind === "conflict") edit.rebase(settled.current);
              setSettled(null);
              setDialog(null);
            }}
            onCloseSettled={() => setSettled(null)}
          />
        </Suspense>
      ) : null}
    </div>
  );
}
