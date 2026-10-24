import { useCallback, useEffect, useState } from "react";
import { applyTheme, readTheme, type ThemeChoice } from "@/lib/prefs/theme";

// The theme switch of the user menu: Light, Dark, or System. /theme-init.js has already set the class from the cookie before
// the first paint; this changes it afterwards and keeps "System" in step with the operating system while the page is open.

export const THEME_CHOICES: readonly { readonly value: ThemeChoice; readonly label: string }[] = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" },
];

export function useTheme(): { choice: ThemeChoice; set: (choice: ThemeChoice) => void } {
  const [choice, setChoice] = useState<ThemeChoice>(() => readTheme());

  useEffect(() => {
    if (choice !== "system" || typeof window.matchMedia !== "function") return undefined;
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const follow = (event: { matches: boolean }): void => {
      document.documentElement.classList.toggle("dark", event.matches);
    };
    query.addEventListener("change", follow);
    return () => {
      query.removeEventListener("change", follow);
    };
  }, [choice]);

  const set = useCallback((next: ThemeChoice) => {
    applyTheme(next);
    setChoice(next);
  }, []);

  return { choice, set };
}
