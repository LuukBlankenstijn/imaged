import { useCallback, useSyncExternalStore } from "react";

type Listener = () => void;

const state = new Map<string, boolean>();
const listeners = new Map<string, Set<Listener>>();

export function setConnection(id: bigint, connected: boolean) {
  const key = id.toString();
  if (state.get(key) === connected) return;
  state.set(key, connected);
  listeners.get(key)?.forEach((fn) => fn());
}

export function clearAllConnections() {
  const keys = Array.from(state.keys());
  state.clear();
  for (const key of keys) {
    listeners.get(key)?.forEach((fn) => fn());
  }
}

function getConnection(id: bigint): boolean {
  return state.get(id.toString()) ?? false;
}

function subscribe(id: bigint, fn: Listener): () => void {
  const key = id.toString();
  let set = listeners.get(key);
  if (!set) {
    set = new Set();
    listeners.set(key, set);
  }
  set.add(fn);
  return () => {
    set!.delete(fn);
    if (set!.size === 0) listeners.delete(key);
  };
}

export function useConnection(id: bigint): boolean {
  return useSyncExternalStore(
    useCallback((cb) => subscribe(id, cb), [id]),
    () => getConnection(id),
    () => false,
  );
}
