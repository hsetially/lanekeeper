/**
 * Shorten text in the middle, so the start (the folder) and the end (the file name) both stay visible.
 * The Style Guide's `trunc`: about 40% from the start, 60% from the end, the ellipsis between, `max` characters in
 * all. Counts code points, so a surrogate pair is never cut in half. The full value goes in a tooltip.
 */
export function truncateMiddle(text: string, max: number): string {
  if (max < 1) return "";
  const chars = Array.from(text);
  if (chars.length <= max) return text;
  const head = Math.ceil((max - 1) * 0.4);
  const tail = Math.floor((max - 1) * 0.6);
  return `${chars.slice(0, head).join("")}…${tail > 0 ? chars.slice(chars.length - tail).join("") : ""}`;
}
