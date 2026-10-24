import { ConsequenceDialog } from "@/components/app/ConsequenceDialog";

// "Cancel edit (asks if there are unsaved changes)": the question, with Keep editing as the safe choice.

export function DiscardDialog({ open, onOpenChange, name, onDiscard }: { open: boolean; onOpenChange: (open: boolean) => void; name: string; onDiscard: () => void }) {
  return (
    <ConsequenceDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Discard your changes?"
      consequence={`Your edits to ${name} have not been saved. Discarding them cannot be undone.`}
      actionLabel="Discard changes"
      cancelLabel="Keep editing"
      onConfirm={() => {
        onOpenChange(false);
        onDiscard();
      }}
      size="sm"
    />
  );
}
