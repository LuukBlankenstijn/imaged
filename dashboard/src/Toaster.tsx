import { dismissToast, useToasts } from "./toast";

export function Toaster() {
  const toasts = useToasts();
  if (toasts.length === 0) return null;
  return (
    <div className="toast-stack" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className={`toast toast-${toast.tone}`}>
          <div className="toast-body">
            <div className="toast-title">{toast.title}</div>
            {toast.message && (
              <div className="toast-message">{toast.message}</div>
            )}
          </div>
          <button
            className="toast-close"
            aria-label="Dismiss"
            onClick={() => dismissToast(toast.id)}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
