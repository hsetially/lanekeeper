import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

// The toasts on screen. A toast is for something the person did not do themselves (a scan saw a file change, a proposal of
// theirs was decided): the result of their own write is shown in the dialog that made it (H-3), never as a toast. So a toast
// holds no secret and no server response, only a title that the caller built and an optional link.
//
// Bounded (rule 5): at most MAX_TOASTS, the oldest dropped. A `key` makes a toast unique: a second one with the same key
// replaces the first (a scan that sees the same file again is one notice). An error never times out.

export const MAX_TOASTS = 5;

export type ToastKind = "info" | "success" | "warning" | "danger";
export type ToastAction = { label: string; href: string } | { label: string; onClick: () => void };

export type ToastInput = {
  kind?: ToastKind;
  /** Another icon than the kind's (the live notice's radio). It stays decorative: the kind is also said in words. */
  icon?: LucideIcon;
  title: ReactNode;
  detail?: ReactNode;
  action?: ToastAction;
  /** Same key, same toast. */
  key?: string;
  /** Milliseconds until it goes by itself, or `null` to stay until dismissed. The default depends on the kind. */
  durationMs?: number | null;
};

export type ToastItem = Omit<ToastInput, "kind" | "durationMs"> & { id: number; kind: ToastKind; durationMs: number | null };

export function defaultDuration(kind: ToastKind): number | null {
  return kind === "danger" ? null : kind === "warning" ? 12_000 : 8_000;
}

export class ToastStore {
  private items: readonly ToastItem[] = [];
  private serial = 0;
  private readonly listeners = new Set<() => void>();

  getSnapshot = (): readonly ToastItem[] => this.items;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  push(input: ToastInput): number {
    const kind = input.kind ?? "info";
    const id = ++this.serial;
    const { kind: _kind, durationMs, ...rest } = input;
    const item: ToastItem = { ...rest, id, kind, durationMs: durationMs === undefined ? defaultDuration(kind) : durationMs };
    const kept = input.key === undefined ? this.items : this.items.filter((t) => t.key !== input.key);
    this.items = [...kept, item].slice(-MAX_TOASTS);
    this.emit();
    return id;
  }

  dismiss(id: number): void {
    if (!this.items.some((t) => t.id === id)) return;
    this.items = this.items.filter((t) => t.id !== id);
    this.emit();
  }

  clear(): void {
    if (this.items.length === 0) return;
    this.items = [];
    this.emit();
  }

  private emit(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

/** The app's toasts. One store for the tab, like the live stream. */
export const toasts = new ToastStore();
export const toast = (input: ToastInput): number => toasts.push(input);
