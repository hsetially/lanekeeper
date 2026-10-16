import { Badge } from "@/components/ui/badge";

/** SIT is outlined and PRESIT is filled muted, so a lane's tier reads without colour. */
export function TierBadge({ tier }: { tier: string }) {
  return <Badge variant={tier.toLowerCase() === "presit" ? "muted" : "outline"}>{tier.toUpperCase()}</Badge>;
}
