import { useSyncExternalStore } from "react";

export type ToastTone = "error" | "info" | "success";

export interface Toast {
  id: number;
  tone: ToastTone;
  title: string;
  message?: string;
}

let nextId = 1;
let toasts: Toast[] = [];
const listeners = new Set<() => void>();

function emit() {
  for (const fn of listeners) fn();
}

export function pushToast(input: Omit<Toast, "id">, ttlMs = 6_000): number {
  const id = nextId++;
  toasts = [...toasts, { id, ...input }];
  emit();
  if (ttlMs > 0) {
    window.setTimeout(() => dismissToast(id), ttlMs);
  }
  return id;
}

export function dismissToast(id: number) {
  const before = toasts.length;
  toasts = toasts.filter((t) => t.id !== id);
  if (toasts.length !== before) emit();
}

export function pushError(title: string, error: unknown) {
  const message = errorMessage(error);
  pushToast({ tone: "error", title, message });
}

export function errorMessage(error: unknown): string {
  if (!error) return "Unknown error";
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function useToasts(): Toast[] {
  return useSyncExternalStore(
    subscribe,
    () => toasts,
    () => toasts,
  );
}
