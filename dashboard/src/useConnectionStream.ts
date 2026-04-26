import { useEffect } from "react";
import { dashboardClient } from "./transport";
import { clearAllConnections, setConnection } from "./connectionStore";

const INITIAL_BACKOFF = 500;
const MAX_BACKOFF = 30_000;
const STABLE_MS = 5_000;

export function useConnectionStream() {
  useEffect(() => {
    const controller = new AbortController();
    let backoff = INITIAL_BACKOFF;

    (async () => {
      while (!controller.signal.aborted) {
        clearAllConnections();
        const startedAt = Date.now();

        try {
          const stream = dashboardClient.connectionState(
            {},
            { signal: controller.signal },
          );
          for await (const ev of stream) {
            setConnection(ev.id, ev.connected);
          }
        } catch (err) {
          if (controller.signal.aborted) return;
          console.warn("connection stream error:", err);
        }

        if (controller.signal.aborted) return;

        if (Date.now() - startedAt > STABLE_MS) {
          backoff = INITIAL_BACKOFF;
        }

        await sleep(backoff, controller.signal);
        backoff = Math.min(backoff * 2, MAX_BACKOFF);
      }
    })();

    return () => controller.abort();
  }, []);
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const t = setTimeout(resolve, ms);
    signal.addEventListener("abort", () => {
      clearTimeout(t);
      resolve();
    });
  });
}
