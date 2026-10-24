// The theme choice (light, dark or follow the system). It is a cookie, not storage, because /theme-init.js must read it
// before the first paint (a classic script cannot wait for a module) and the hub could one day read it too. It is not a
// secret and not a session: a name from a list of three.

export const THEME_COOKIE = "lk_theme";
export type ThemeChoice = "light" | "dark" | "system";
const CHOICES: readonly ThemeChoice[] = ["light", "dark", "system"];

export function parseThemeCookie(cookies: string): ThemeChoice {
  for (const pair of cookies.split(";")) {
    const eq = pair.indexOf("=");
    if (eq < 0 || pair.slice(0, eq).trim() !== THEME_COOKIE) continue;
    const value = pair.slice(eq + 1).trim();
    return CHOICES.find((c) => c === value) ?? "system";
  }
  return "system";
}

export function themeCookieValue(choice: ThemeChoice): string {
  return `${THEME_COOKIE}=${choice}; Path=/; Max-Age=31536000; SameSite=Lax; Secure`;
}

export function readTheme(): ThemeChoice {
  return typeof document === "undefined" ? "system" : parseThemeCookie(document.cookie);
}

/** The class on <html> that the styles key on, now. */
export function applyTheme(choice: ThemeChoice): void {
  if (typeof document === "undefined") return;
  document.cookie = themeCookieValue(choice);
  const dark = choice === "dark" || (choice === "system" && typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}
