import { useEffect, useState } from "react";

/**
 * The current time in milliseconds, refreshed every `intervalMs` while `active` (a countdown). While it is not active it does
 * not tick and no timer exists, so a page whose stream is live pays nothing for it.
 */
export function useNow(active: boolean, intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return undefined;
    setNow(Date.now());
    const id = setInterval(() => {
      setNow(Date.now());
    }, intervalMs);
    return () => {
      clearInterval(id);
    };
  }, [active, intervalMs]);
  return now;
}
