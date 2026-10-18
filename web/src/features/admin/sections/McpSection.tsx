import { PersonalTokens } from "@/components/app/PersonalTokens";

// MCP personal tokens, on the Admin side. The section mounts the shared PersonalTokens component (the same one as the account page,
// S1's /me/tokens), which lists, makes and revokes the SIGNED-IN person's own tokens, shows a new token once and keeps it in its own
// state only, and says "not switched on" when the hub answers the list with 404. The design draws every user's tokens with an owner,
// issues a token for another user and lets an Admin revoke any: the contract has only "my tokens" operations, so none of that is
// here (design/DEVIATIONS.md).

export function McpSection() {
  return (
    <div className="flex flex-col gap-3">
      <p className="max-w-[680px] text-sm text-muted-foreground">
        These are your own tokens. A token lets an agent in an editor, such as Cursor, act as you: it can propose changes, and an Admin other than the owner approves each one. The hub offers no way for an Admin to list or revoke other people's tokens from here.
      </p>
      <div className="rounded-md border border-border bg-card">
        <PersonalTokens heading="Your tokens" />
      </div>
    </div>
  );
}
