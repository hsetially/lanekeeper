import type { ReactNode } from "react";
import type { ChangePreview, PreviewWarning } from "@/api/generated/model";
import { SafeText } from "@/components/app/SafeText";
import { Alert } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { formatTime } from "@/lib/time";
import { cn } from "@/lib/utils";

// What an upload or a delete would do, from the engine, before anybody confirms (D46, D83, D85). The engine computed the scope,
// the warnings and the settings that would differ in the effective config; this words them. A value the secret scan flagged is
// already its marker; nothing here can unmask it. The preview is bound to the user, the swimlane, the path and the hashes, and it
// expires: the dialog that shows it says until when.

type WordedWarning = { tone: "info" | "warning" | "danger"; text: string };

function wording(warning: PreviewWarning): WordedWarning {
  switch (warning.kind) {
    case "falls_back_to_base":
      return { tone: "warning", text: "This tenant file is removed, so the services fall back to the base file." };
    case "no_file_left":
      return { tone: "danger", text: "No file is left for these services: nothing will define these settings." };
    case "forks_from_base":
      return { tone: "warning", text: "This creates a tenant file that forks from the base file: later base changes will not reach it." };
    case "needs_notify_or_restart":
      return { tone: "info", text: "Services pick this change up after a notify or a restart." };
    case "new_folder_needs_config_server_restart":
      return { tone: "warning", text: "A new folder needs the config server to restart before it is read." };
    case "channels_yml_needs_config_server_restart":
      return { tone: "warning", text: "channels.yml is read when the config server starts: it needs a restart." };
    case "never_read":
      return { tone: "warning", text: `No swimlane reads a file with the suffix "${warning.suffix}": this file would never be used.` };
  }
}

function scopeText(scope: ChangePreview["scope"]): string {
  switch (scope.kind) {
    case "every_service":
      return "Every service reads this file.";
    case "application":
      return `The application ${scope.app} reads this file.`;
    case "folder":
      return `Services that read the folder ${scope.path} read this file.`;
  }
}

const KIND = { added: "Added", removed: "Removed", changed: "Changed", type_changed: "Changed" } as const;
const KIND_CLASS = { added: "text-diff-add-fg", removed: "text-diff-del-fg", changed: "text-diff-mod-fg", type_changed: "text-diff-mod-fg" } as const;

function Value({ text, none }: { text: string | undefined; none: string }): ReactNode {
  return text === undefined ? <span className="text-muted-foreground">{none}</span> : <SafeText text={text} className="font-mono" />;
}

export function EffectPreview({ preview }: { preview: ChangePreview }) {
  const hidden = Math.max(0, preview.changes_total - preview.changes.length);
  return (
    <div className="flex flex-col gap-3">
      {preview.warnings.map((warning, i) => {
        const { tone, text } = wording(warning);
        return (
          <Alert key={i} variant={tone}>
            <SafeText text={text} />
          </Alert>
        );
      })}
      <p className="m-0 text-sm text-muted-foreground">{scopeText(preview.scope)}</p>
      {preview.changes.length > 0 ? (
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">What changes in the effective config</span>
          <table aria-label="What changes in the effective config" className="w-full border-collapse overflow-hidden rounded-md border border-border text-xs">
            <thead>
              <tr className="bg-muted text-left text-muted-foreground">
                <th scope="col" className="w-[72px] px-2 py-1 font-medium">
                  Change
                </th>
                <th scope="col" className="px-2 py-1 font-medium">
                  Key
                </th>
                <th scope="col" className="px-2 py-1 font-medium">
                  Now
                </th>
                <th scope="col" className="px-2 py-1 font-medium">
                  After
                </th>
              </tr>
            </thead>
            <tbody>
              {preview.changes.map((item, i) => (
                <tr key={i} className="border-t border-border align-top">
                  <td className={cn("px-2 py-1 font-medium", KIND_CLASS[item.change.kind])}>{KIND[item.change.kind]}</td>
                  <td className="px-2 py-1 font-mono [overflow-wrap:anywhere]">
                    <SafeText text={item.change.path} />
                    {item.flagged ? (
                      <Badge variant="muted" className="ml-1.5">
                        Masked
                      </Badge>
                    ) : null}
                  </td>
                  <td className="px-2 py-1">
                    <Value text={item.change.left?.text} none="not set" />
                  </td>
                  <td className="px-2 py-1">
                    <Value text={item.change.right?.text} none="not set" />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {hidden > 0 ? <span className="text-xs text-muted-foreground">{`and ${hidden} more ${hidden === 1 ? "change is" : "changes are"} not listed.`}</span> : null}
        </div>
      ) : (
        <p className="m-0 text-sm">{preview.effective_changed ? "The effective config changes." : "The effective config does not change."}</p>
      )}
      <p className="m-0 text-xs text-muted-foreground">This preview is good until {formatTime(preview.expires_at)}.</p>
    </div>
  );
}
