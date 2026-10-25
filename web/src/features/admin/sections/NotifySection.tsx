import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { NotificationChannel, NotificationChannelInput } from "@/api/generated/model";
import { EmptyState } from "@/components/app/EmptyState";
import { QueryBoundaryView } from "@/components/app/QueryBoundary";
import { SafeText } from "@/components/app/SafeText";
import { visibleText } from "@/components/app/safe-text";
import type { Column } from "@/components/app/virtual/DataTable";
import { WriteError, type WriteErrorInfo } from "@/components/app/WriteError";
import { useWriteGate } from "@/components/app/write-gate";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { AdminTable } from "@/features/admin/AdminTable";
import { adminMutations, afterAdminSecretWrite, putNotificationRequest } from "@/features/admin/mutations";
import { adminQueries } from "@/features/admin/queries";
import { DeleteRuleDialog, RuleActions, RulesHeader, SavedNotice, type DeleteTarget } from "@/features/admin/rules";
import { adminWriteError } from "@/features/admin/write-error";
import { Bell, Lock } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { useSecretMutation } from "@/lib/query/secret-mutation";
import { cursorPagesSource, useQueryState } from "@/lib/query/use-query-state";
import { ChannelDialog, KIND_LABELS, severityLabel } from "./ChannelDialog";

// Notifications: the Microsoft Teams channels that carry events. The webhook of a channel is a secret that the hub never returns, so
// the list says only whether one is set (ChannelDialog has the rules for it). Switching a channel on or off sends the channel's own
// fields without any address, and goes through the secret path too, because it is the same operation. Nothing is shown before the
// hub answers. The design's "Send test" has no operation, so it is not drawn.

const plural = (n: number, one: string, many: string): string => (n === 1 ? one : many);

export function NotifySection() {
  const client = useQueryClient();
  const gate = useWriteGate("admin");
  const pages = useCursorPages({ ...adminQueries.notifications(), autoLoad: true });
  const state = useQueryState(cursorPagesSource(pages), { isEmpty: (items) => items.length === 0 });
  const remove = useMutation(adminMutations.deleteNotification(client));
  const flip = useSecretMutation((variables: { id: string; body: NotificationChannelInput }) => putNotificationRequest(variables.id, variables.body));
  const [editing, setEditing] = useState<{ channel: NotificationChannel | null } | null>(null);
  const [deleting, setDeleting] = useState<DeleteTarget | null>(null);
  const [saved, setSaved] = useState<{ name: string; urlSent: boolean } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [refused, setRefused] = useState<WriteErrorInfo | null>(null);

  const switchChannel = (channel: NotificationChannel, enabled: boolean): void => {
    setSaved(null);
    setRefused(null);
    setBusy(channel.id);
    flip
      .run({ id: channel.id, body: { name: channel.name, min_severity: channel.min_severity, kinds: channel.kinds, enabled } })
      .then(() => afterAdminSecretWrite(client, "adminPutNotification"))
      .catch((error: unknown) => {
        setRefused({ ...adminWriteError(error, `${channel.name} could not be switched`) });
      })
      .finally(() => {
        setBusy(null);
      });
  };

  const columns: Column<NotificationChannel>[] = [
    {
      id: "name",
      header: "Channel",
      width: "minmax(0,1.4fr)",
      minWidth: 180,
      cell: (channel) => (
        <span className="flex min-w-0 items-baseline gap-2">
          <b className="truncate font-medium">
            <SafeText text={channel.name} />
          </b>
          <SafeText text={channel.id} className="truncate font-mono text-xs text-muted-foreground" />
        </span>
      ),
    },
    {
      id: "events",
      header: "Events",
      width: "96px",
      minWidth: 96,
      cell: (channel) => (
        <span title={channel.kinds.map((kind) => KIND_LABELS[kind]).join(", ")}>
          {channel.kinds.length} {plural(channel.kinds.length, "event", "events")}
        </span>
      ),
    },
    { id: "severity", header: "From severity", width: "104px", minWidth: 104, cell: (channel) => <span>{severityLabel(channel.min_severity)}</span> },
    {
      id: "webhook",
      header: "Webhook",
      width: "136px",
      minWidth: 136,
      cell: (channel) =>
        channel.webhook_configured ? (
          <Badge title="Saved. It can't be shown again.">
            <Lock aria-hidden="true" />
            Webhook set
          </Badge>
        ) : (
          <Badge variant="muted">Not set</Badge>
        ),
    },
    {
      id: "enabled",
      header: "On",
      width: "56px",
      minWidth: 56,
      cell: (channel) => (
        <Switch
          aria-label={`Send ${visibleText(channel.name)}`}
          checked={channel.enabled}
          disabled={!gate.allowed || busy === channel.id}
          {...(gate.reason === null ? {} : { title: gate.reason })}
          onCheckedChange={(next) => {
            switchChannel(channel, next);
          }}
        />
      ),
    },
    {
      id: "actions",
      header: <span className="sr-only">Actions</span>,
      width: "72px",
      minWidth: 72,
      align: "right",
      cell: (channel) => (
        <RuleActions
          what={`channel ${visibleText(channel.id)}`}
          onEdit={() => {
            setSaved(null);
            setRefused(null);
            setEditing({ channel });
          }}
          onDelete={() => {
            setSaved(null);
            setRefused(null);
            setDeleting({ id: channel.id, what: `channel ${visibleText(channel.id)}` });
          }}
        />
      ),
    },
  ];

  const off = state.state === "empty" || (state.state === "ready" && !pages.items.some((channel) => channel.webhook_configured && channel.enabled));
  const count = state.state === "ready" ? `${pages.items.length} ${plural(pages.items.length, "channel", "channels")}` : "";
  return (
    <div className="flex flex-col gap-3">
      {off ? (
        <Alert variant="info">
          <AlertTitle>Notifications are off</AlertTitle>
          <AlertDescription>No channel has a webhook that is switched on. Events show in Lanekeeper only.</AlertDescription>
        </Alert>
      ) : null}
      <RulesHeader
        title="Teams channels"
        count={count}
        addLabel="Add channel"
        onAdd={() => {
          setSaved(null);
          setRefused(null);
          setEditing({ channel: null });
        }}
      />
      {saved === null ? null : (
        <SavedNotice
          title={`Channel ${visibleText(saved.name)} saved`}
          onDismiss={() => {
            setSaved(null);
          }}
        >
          {saved.urlSent ? "The webhook was saved. It is masked from now on and can't be shown again." : "The webhook already saved is kept."}
        </SavedNotice>
      )}
      {refused === null ? null : <WriteError {...refused} />}
      <QueryBoundaryView
        state={state}
        noun="notification channels"
        source="the notification channels"
        empty={
          <div className="rounded-md border border-border bg-card">
            <EmptyState icon={Bell} title="No notification channels">
              Add a channel to send events to a Microsoft Teams channel.
            </EmptyState>
          </div>
        }
      >
        {(channels) => <AdminTable label="Notification channels" rows={channels} columns={columns} rowKey={(channel) => channel.id} />}
      </QueryBoundaryView>
      {editing === null ? null : (
        <ChannelDialog
          key={editing.channel?.id ?? "new"}
          channel={editing.channel}
          onSaved={(name, urlSent) => {
            setSaved({ name, urlSent });
          }}
          onClose={() => {
            setEditing(null);
          }}
        />
      )}
      <DeleteRuleDialog
        target={deleting}
        title={deleting === null ? "Delete channel" : `Delete ${deleting.what}`}
        consequence="Events stop going to this Teams channel. The webhook is forgotten and can't be recovered."
        action="Delete channel"
        mutation={remove}
        onClose={() => {
          setDeleting(null);
        }}
      />
    </div>
  );
}
