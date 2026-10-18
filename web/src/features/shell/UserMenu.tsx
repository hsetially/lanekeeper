import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Role, type Me } from "@/api/generated/model";
import { roleAtLeast, roleLabel } from "@/auth/roles";
import { useSignOut } from "@/auth/signout";
import { SafeText } from "@/components/app/SafeText";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuSeparator, DropdownMenuShortcut, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { githubCredentialQuery, mcpTokensQuery } from "@/features/access/queries";
import { ChevronsUpDown, Eye, Key, Keyboard, LogOut, Pencil, RotateCw, Server, ShieldCheck } from "@/icons";
import { useCursorPages } from "@/lib/query/cursor-pages";
import type { ThemeChoice } from "@/lib/prefs/theme";
import { cn } from "@/lib/utils";
import { AppLink } from "@/routes/AppLink";
import { useShell } from "./ShellContext";
import { THEME_CHOICES, useTheme } from "./theme";
import { toast } from "./toast/store";

// The account block at the foot of the sidebar and its menu (the Shell, frames 1a and 1g): who is signed in and with which
// role; My GitHub token (Editor and above, with the days left when GitHub said); Keyboard shortcuts; the theme (Light, Dark,
// System); Sign out. The role is shown in words and by an icon, never by colour alone.

const ROLE_ICON = { [Role.admin]: ShieldCheck, [Role.operator]: RotateCw, [Role.editor]: Pencil, [Role.viewer]: Eye } as const;

/** `PS` for Priya Shah: the first letter of the first two words. Whatever the name holds, the result is at most two characters. */
export function initials(name: string): string {
  const letters = name
    .trim()
    .split(/\s+/)
    .filter((word) => word !== "")
    .slice(0, 2)
    .map((word) => Array.from(word)[0] ?? "");
  return letters.join("").toUpperCase() || "?";
}

/** Whole days from `now` to `at`, rounded up; 0 for today and for a time that has passed. */
export function daysLeft(at: number, now: number): number {
  return Math.max(0, Math.ceil((at - now) / 86_400_000));
}

function RoleChip({ role, filled }: { role: Role; filled: boolean }) {
  const Icon = ROLE_ICON[role];
  return (
    <span
      data-slot="role-chip"
      className={cn("inline-flex h-[18px] items-center gap-1 rounded-sm border px-1.5 text-xs font-medium", filled ? "border-foreground bg-foreground text-background" : "border-input text-foreground")}
    >
      <Icon aria-hidden="true" className="size-3" />
      {roleLabel(role)}
    </span>
  );
}

function CredentialNote({ open }: { open: boolean }) {
  const credential = useQuery({ ...githubCredentialQuery(), enabled: open });
  const data = credential.data;
  if (!data) return null;
  if (data.expires_at === null || data.expires_at === undefined) return null;
  const left = daysLeft(data.expires_at, Date.now());
  const text = left === 0 && data.expires_at <= Date.now() ? "Expired" : left === 1 ? "1 day left" : `${left} days left`;
  return <span className={cn("ml-auto text-xs", data.expires_soon === true ? "font-medium text-warning-fg" : "text-muted-foreground")}>{text}</span>;
}

export function UserMenu({ me, role, collapsed }: { me: Me; role: Role | null; collapsed: boolean }) {
  const [open, setOpen] = useState(false);
  const { openShortcuts } = useShell();
  const theme = useTheme();
  const { signOut, problem } = useSignOut();
  const after = useRef<(() => void) | null>(null);
  const canToken = roleAtLeast(role, Role.editor);
  // Personal MCP tokens exist only when the hub has the mode switched on; it says so by answering the list (a 404 when not).
  // Asked when the menu opens, so that a page that never opens the menu never asks.
  const mcp = useCursorPages({ ...mcpTokensQuery(), enabled: open && role !== null });
  const hasMcp = mcp.status === "success";

  useEffect(() => {
    if (problem) toast({ key: "sign-out", kind: "danger", title: "Could not sign out", detail: problem.title });
  }, [problem]);

  const name = me.display_name;
  const trigger = collapsed ? (
    <Tooltip>
      <TooltipTrigger asChild>
        <DropdownMenuTrigger asChild>
          <button type="button" aria-label={`${name}, account menu`} className="flex h-9 w-full cursor-pointer items-center justify-center rounded-sm border border-transparent hover:bg-accent data-[state=open]:bg-accent">
            <Avatar name={name} />
          </button>
        </DropdownMenuTrigger>
      </TooltipTrigger>
      <TooltipContent side="right">{name}</TooltipContent>
    </Tooltip>
  ) : (
    <DropdownMenuTrigger asChild>
      <button type="button" className="flex h-12 w-full cursor-pointer items-center gap-2 rounded-sm border border-transparent px-2 text-left hover:bg-accent data-[state=open]:bg-accent">
        <Avatar name={name} />
        <span className="flex min-w-0 flex-1 flex-col items-start gap-0.5">
          <SafeText text={name} className="max-w-full truncate font-medium" />
          {role === null ? <span className="inline-flex h-[18px] items-center rounded-sm border border-input px-1.5 text-xs font-medium">No role</span> : <RoleChip role={role} filled={role === Role.admin} />}
        </span>
        <ChevronsUpDown aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
      </button>
    </DropdownMenuTrigger>
  );

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      {trigger}
      <DropdownMenuContent
        side="top"
        align="start"
        sideOffset={8}
        className="w-[248px]"
        onCloseAutoFocus={(event) => {
          // An item that opens a dialog runs once the menu has closed and given focus back, or the two fight over it.
          const run = after.current;
          if (run) {
            event.preventDefault();
            after.current = null;
            run();
          }
        }}
      >
        <div className="mb-1 flex h-9 items-center justify-between gap-2 border-b border-border px-2">
          <SafeText text={name} className="truncate font-semibold" />
          <span className="shrink-0 text-xs text-muted-foreground">{role === null ? "No role" : roleLabel(role)}</span>
        </div>
        {canToken ? (
          <DropdownMenuItem asChild>
            <AppLink route="github-token">
              <Key aria-hidden="true" className="text-muted-foreground" />
              <span>My GitHub token</span>
              <CredentialNote open={open} />
            </AppLink>
          </DropdownMenuItem>
        ) : null}
        {hasMcp ? (
          <DropdownMenuItem asChild>
            <AppLink route="mcp-tokens">
              <Server aria-hidden="true" className="text-muted-foreground" />
              <span>Personal MCP tokens</span>
            </AppLink>
          </DropdownMenuItem>
        ) : null}
        <DropdownMenuItem
          onSelect={() => {
            after.current = openShortcuts;
          }}
        >
          <Keyboard aria-hidden="true" className="text-muted-foreground" />
          <span>Keyboard shortcuts</span>
          <DropdownMenuShortcut>?</DropdownMenuShortcut>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Theme</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={theme.choice}
          onValueChange={(value) => {
            theme.set(value as ThemeChoice);
          }}
        >
          {THEME_CHOICES.map((choice) => (
            <DropdownMenuRadioItem key={choice.value} value={choice.value} onSelect={(event) => { event.preventDefault(); }}>
              {choice.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={() => {
            void signOut();
          }}
        >
          <LogOut aria-hidden="true" className="text-muted-foreground" />
          <span>Sign out</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function Avatar({ name }: { name: string }) {
  return (
    <span aria-hidden="true" data-slot="avatar" className="inline-flex size-7 shrink-0 items-center justify-center rounded-full border border-input bg-secondary text-xs font-semibold">
      {initials(name)}
    </span>
  );
}
