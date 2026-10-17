import { SignOutButton } from "@/auth/SignOutButton";
import { useSession } from "@/auth/session";
import { LiveIndicator } from "@/components/app/LiveIndicator";
import { PausedBanner } from "@/components/app/PausedBanner";
import { RoleBadge } from "@/components/app/RoleBadge";
import { SafeText } from "@/components/app/SafeText";

// The minimum chrome until the shell slice (T4, S0) builds the designed one: who is signed in, the live stream's pill and
// banner, and a way out. Shown only when the session is known; a screen before sign-in has no header.
export function Frame() {
  const session = useSession();
  if (session.status !== "ready") return null;
  return (
    <>
      <header className="flex items-center justify-end gap-3 border-b border-border px-4 py-2">
        <LiveIndicator />
        <SafeText text={session.me.display_name} />
        <RoleBadge role={session.role} />
        <SignOutButton />
      </header>
      <PausedBanner />
    </>
  );
}
