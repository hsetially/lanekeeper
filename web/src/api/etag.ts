// A bounded cache of response bodies, keyed by URL, that lets the client send `If-None-Match` and use the kept body when
// the answer is 304 (F5: the hub marks API responses `no-store`, so the browser never revalidates for us).
//
// Bounded in entries and in bytes (rule 5), least recently used out first. It keeps the body as text, so a 304 gives a
// fresh object each time and nothing a caller does to it reaches the next caller. It is emptied on a 401 and on sign-out.

export type EtagLimits = { maxEntries: number; maxBytes: number };

export const DEFAULT_ETAG_LIMITS: EtagLimits = { maxEntries: 200, maxBytes: 8 * 1024 * 1024 };

type Entry = { etag: string; text: string; bytes: number };

export class EtagCache {
  readonly limits: EtagLimits;
  private readonly entries = new Map<string, Entry>();
  private total = 0;

  constructor(limits: Partial<EtagLimits> = {}) {
    this.limits = { ...DEFAULT_ETAG_LIMITS, ...limits };
  }

  get size(): number {
    return this.entries.size;
  }

  get bytes(): number {
    return this.total;
  }

  get(key: string): { etag: string; text: string } | undefined {
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    // Re-insert so the Map's order is the order of use.
    this.entries.delete(key);
    this.entries.set(key, entry);
    return { etag: entry.etag, text: entry.text };
  }

  set(key: string, etag: string, text: string): void {
    this.delete(key);
    // JavaScript strings are UTF-16; two bytes a unit is what the body costs in memory.
    const bytes = text.length * 2;
    // One body that takes more than half of the budget is not worth evicting the rest for.
    if (bytes > this.limits.maxBytes / 2) return;
    this.entries.set(key, { etag, text, bytes });
    this.total += bytes;
    while (this.entries.size > this.limits.maxEntries || this.total > this.limits.maxBytes) {
      const oldest = this.entries.keys().next();
      if (oldest.done) break;
      this.delete(oldest.value);
    }
  }

  delete(key: string): void {
    const entry = this.entries.get(key);
    if (!entry) return;
    this.entries.delete(key);
    this.total -= entry.bytes;
  }

  clear(): void {
    this.entries.clear();
    this.total = 0;
  }
}
