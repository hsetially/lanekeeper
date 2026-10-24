import { useQueryClient } from "@tanstack/react-query";
import { visibleText } from "@/components/app/safe-text";
import { useId, useState } from "react";
import { NotificationKind, Severity, type NotificationChannel, type NotificationChannelInput } from "@/api/generated/model";
import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";
import type { WriteErrorInfo } from "@/components/app/WriteError";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldControl, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";
import { afterAdminSecretWrite, putNotificationRequest } from "@/features/admin/mutations";
import { checkConfigKey, checkName, checkWebhook, omitKey } from "@/features/admin/validate";
import { adminWriteError } from "@/features/admin/write-error";
import { Eye } from "@/icons";
import { useSecretMutation } from "@/lib/query/secret-mutation";

// One notification channel: a Microsoft Teams webhook that carries the events chosen, from a minimum severity, on or off.
//
// THE WEBHOOK URL IS A SECRET (S21). It is typed into this dialog, sent once, and the hub never returns it, so the list can only say
// whether one is set. It lives in this component's state and nowhere else: not in the query cache, not in the mutation cache
// (`useSecretMutation`: gcTime 0, removed and reset in a `finally`, never retried), not in storage, not in a URL, not in a log. It is
// hidden as it is typed (the person can ask to see it), emptied after EVERY answer, success or refusal, and gone with the dialog.
// A refusal therefore asks for it again. Nothing the page says about a wrong address repeats it.
//
// An existing channel can be saved without a new address: the body then has no `webhook_url`, and the contract says "omit it to keep
// the stored one" (adminPutNotification), so the page leaves it out unless a new one was typed.

export const KIND_LABELS: Readonly<Record<NotificationKind, string>> = {
  access_requested: "Access requested",
  proposal_pending: "Proposal waiting for approval",
  proposal_decided: "Proposal decided",
  service_restarted: "Service restarted",
  pr_raised: "Pull request raised",
  agent_disconnected: "Agent disconnected",
  critical_finding: "Critical finding",
  drift_detected: "Drift detected",
  sync_window_stale: "Sync window stale",
};
export const KIND_ORDER: readonly NotificationKind[] = Object.values(NotificationKind);
export const SEVERITIES: readonly Severity[] = Object.values(Severity);
export const severityLabel = (severity: Severity): string => severity.charAt(0).toUpperCase() + severity.slice(1);

type Errors = { id?: string; name?: string; kinds?: string; url?: string };

export function ChannelDialog({ channel, onClose, onSaved }: { channel: NotificationChannel | null; onClose: () => void; onSaved: (name: string, urlSent: boolean) => void }) {
  const client = useQueryClient();
  const put = useSecretMutation((variables: { id: string; body: NotificationChannelInput }) => putNotificationRequest(variables.id, variables.body));
  const uid = useId();
  const editing = channel !== null;
  const [id, setId] = useState(channel?.id ?? "");
  const [name, setName] = useState(channel?.name ?? "");
  const [kinds, setKinds] = useState<NotificationKind[]>(channel ? [...channel.kinds] : []);
  const [severity, setSeverity] = useState<Severity>(channel?.min_severity ?? Severity.medium);
  const [enabled, setEnabled] = useState(channel?.enabled ?? true);
  // The secret. Everything below that handles it clears it.
  const [url, setUrl] = useState("");
  const [shown, setShown] = useState(false);
  const [cleared, setCleared] = useState(false);
  const [errors, setErrors] = useState<Errors>({});
  const [working, setWorking] = useState(false);
  const [failure, setFailure] = useState<WriteErrorInfo | null>(null);

  const submit = (): void => {
    const found: Errors = {};
    if (!editing) {
      const bad = checkConfigKey(id.trim(), "Enter a channel id");
      if (bad !== null) found.id = bad;
    }
    const badName = checkName(name);
    if (badName !== null) found.name = badName;
    if (kinds.length === 0) found.kinds = "Choose at least one event";
    const typed = url.trim();
    if (!editing || typed !== "") {
      const badUrl = checkWebhook(typed);
      if (badUrl !== null) found.url = badUrl;
    }
    setErrors(found);
    if (Object.keys(found).length > 0) return;

    const body: NotificationChannelInput = { name: name.trim(), min_severity: severity, kinds, enabled, ...(typed === "" ? {} : { webhook_url: typed }) };
    const channelId = editing ? channel.id : id.trim();
    setFailure(null);
    setWorking(true);
    put
      .run({ id: channelId, body })
      .then(async () => {
        setUrl("");
        await afterAdminSecretWrite(client, "adminPutNotification");
        onSaved(body.name, typed !== "");
        onClose();
      })
      .catch((error: unknown) => {
        setUrl("");
        setShown(false);
        setCleared(typed !== "");
        setFailure(adminWriteError(error, "The channel could not be saved"));
      })
      .finally(() => {
        setWorking(false);
      });
  };

  return (
    <ConsequenceDialog
      initialFocus="first-field"
      open
      onOpenChange={(open) => {
        if (!open) {
          setUrl("");
          onClose();
        }
      }}
      title={editing ? `Edit channel ${visibleText(channel.id)}` : "Add a notification channel"}
      consequence="Events of the kinds chosen, at or above the minimum severity, are sent to this Teams channel as soon as it is on and has a webhook."
      actionLabel="Save channel"
      workingLabel="Saving…"
      size="lg"
      working={working}
      error={failure}
      onConfirm={submit}
    >
      <div className="grid grid-cols-2 gap-3 max-sm:grid-cols-1">
        <Field invalid={errors.id !== undefined}>
          <FieldLabel>Channel id</FieldLabel>
          <FieldControl>
            <Input value={id} readOnly={editing} autoComplete="off" spellCheck={false} placeholder="approvals" className="font-mono" onChange={(event) => setId(event.target.value)} />
          </FieldControl>
          <FieldError>{errors.id}</FieldError>
          {editing ? <FieldDescription>The id cannot change.</FieldDescription> : null}
        </Field>
        <Field invalid={errors.name !== undefined}>
          <FieldLabel>Name</FieldLabel>
          <FieldControl>
            <Input value={name} autoComplete="off" placeholder="Approvals" onChange={(event) => setName(event.target.value)} />
          </FieldControl>
          <FieldError>{errors.name}</FieldError>
        </Field>
      </div>

      <fieldset className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0">
        <legend className="mb-1 text-sm font-medium">Events</legend>
        <ul className="m-0 grid list-none grid-cols-2 gap-x-4 gap-y-1.5 p-0 max-sm:grid-cols-1">
          {KIND_ORDER.map((kind) => (
            <li key={kind} className="flex items-center gap-2">
              <Checkbox
                id={`${uid}-${kind}`}
                checked={kinds.includes(kind)}
                onCheckedChange={(checked) => {
                  setKinds(checked === true ? [...kinds, kind] : kinds.filter((k) => k !== kind));
                  setErrors(omitKey(errors, "kinds"));
                }}
              />
              <Label htmlFor={`${uid}-${kind}`} className="font-normal">
                {KIND_LABELS[kind]}
              </Label>
            </li>
          ))}
        </ul>
        {errors.kinds === undefined ? null : (
          <p role="alert" className="text-xs text-danger-fg">
            {errors.kinds}
          </p>
        )}
      </fieldset>

      <div className="grid grid-cols-2 items-end gap-3 max-sm:grid-cols-1">
        <Field>
          <FieldLabel>Minimum severity</FieldLabel>
          <FieldControl>
            <NativeSelect
              value={severity}
              onChange={(event) => {
                setSeverity(SEVERITIES.find((s) => s === event.target.value) ?? Severity.medium);
              }}
            >
              {SEVERITIES.map((s) => (
                <option key={s} value={s}>
                  {severityLabel(s)}
                </option>
              ))}
            </NativeSelect>
          </FieldControl>
        </Field>
        <div className="flex h-7 items-center gap-2">
          <Switch id={`${uid}-enabled`} checked={enabled} onCheckedChange={setEnabled} />
          <Label htmlFor={`${uid}-enabled`} className="font-normal">
            Send events to this channel
          </Label>
        </div>
      </div>

      <Field invalid={errors.url !== undefined}>
        <FieldLabel>Webhook URL</FieldLabel>
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <FieldControl>
              <Input
                type={shown ? "text" : "password"}
                value={url}
                autoComplete="new-password"
                spellCheck={false}
                placeholder="https://…webhook.office.com/webhookb2/…"
                className="font-mono"
                onChange={(event) => {
                  setUrl(event.target.value);
                  setCleared(false);
                }}
              />
            </FieldControl>
          </div>
          <Button
            variant="secondary"
            aria-pressed={shown}
            aria-label={shown ? "Hide webhook URL" : "Show webhook URL"}
            onClick={() => {
              setShown(!shown);
            }}
          >
            <Eye data-icon="inline-start" />
            {shown ? "Hide" : "Show"}
          </Button>
        </div>
        <FieldError>{errors.url}</FieldError>
        <FieldDescription>
          {cleared
            ? "Enter the webhook URL again. It is cleared after every answer, so that it is not kept."
            : editing
              ? "Leave empty to keep the saved webhook. A new address replaces it. Once saved it is masked and can't be shown again."
              : "Shown in full only while you ask to see it. Once saved it is masked and can't be shown again."}
        </FieldDescription>
      </Field>
    </ConsequenceDialog>
  );
}
