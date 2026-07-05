import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Host } from "@imaged/gen/v1/dashboard/host_pb";
import type { Task } from "@imaged/gen/v1/dashboard/task_pb";
import { TaskState, TaskType } from "@imaged/gen/v1/dashboard/task_pb";
import { dashboardClient } from "./transport";
import { formatRelative, timestampToDate } from "./format";

type TypeFilter = "all" | "capture" | "deploy" | "multicast" | "reboot";
type StatusFilter = "active" | "all" | "completed";

const ACTIVE_STATES = new Set<TaskState>([
  TaskState.TASK_PENDING,
  TaskState.TASK_RUNNING,
]);

const RETRYABLE_STATES = new Set<TaskState>([
  TaskState.TASK_FAILED,
  TaskState.TASK_CANCELLED,
  TaskState.TASK_PARTIAL,
]);

export function TasksView() {
  const [typeFilter, setTypeFilter] = useState<TypeFilter>("all");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("active");
  const [hostFilter, setHostFilter] = useState<string>("all");

  const tasksQuery = useQuery({
    queryKey: ["tasks"],
    queryFn: () => dashboardClient.getAllTasks({}),
    refetchInterval: 1_500,
  });

  const hostsQuery = useQuery({
    queryKey: ["hosts"],
    queryFn: () => dashboardClient.getAllHosts({}),
  });

  const hostsById = useMemo(() => {
    const map = new Map<string, Host>();
    for (const h of hostsQuery.data?.hosts ?? []) map.set(h.id.toString(), h);
    return map;
  }, [hostsQuery.data]);

  const tasks = tasksQuery.data?.tasks ?? [];

  const filtered = useMemo(() => {
    return tasks
      .filter((t) => {
        if (typeFilter === "capture" && t.type !== TaskType.TYPE_CAPTURE) {
          return false;
        }
        if (typeFilter === "deploy" && t.type !== TaskType.TYPE_DEPLOY) {
          return false;
        }
        if (typeFilter === "multicast" && t.type !== TaskType.TYPE_MULTICAST) {
          return false;
        }
        if (typeFilter === "reboot" && t.type !== TaskType.TYPE_REBOOT) {
          return false;
        }
        const active = ACTIVE_STATES.has(t.state);
        if (statusFilter === "active" && !active) return false;
        if (statusFilter === "completed" && active) return false;
        if (hostFilter !== "all") {
          if (!t.hosts.some((h) => h.hostId.toString() === hostFilter)) {
            return false;
          }
        }
        return true;
      })
      .sort((a, b) => createdMillis(b) - createdMillis(a));
  }, [tasks, typeFilter, statusFilter, hostFilter]);

  const totalActive = useMemo(
    () => tasks.filter((t) => ACTIVE_STATES.has(t.state)).length,
    [tasks],
  );

  return (
    <>
      <header className="page-head">
        <h1 className="page-title">Tasks</h1>
        <span className="page-meta">
          <strong>{totalActive}</strong> active
        </span>
      </header>

      <div className="filter-bar">
        <div className="filter-group">
          <label className="filter-label">Status</label>
          <div className="segmented">
            <button
              className={statusFilter === "active" ? "seg active" : "seg"}
              onClick={() => setStatusFilter("active")}
            >
              Active
            </button>
            <button
              className={statusFilter === "all" ? "seg active" : "seg"}
              onClick={() => setStatusFilter("all")}
            >
              All
            </button>
            <button
              className={statusFilter === "completed" ? "seg active" : "seg"}
              onClick={() => setStatusFilter("completed")}
            >
              Completed
            </button>
          </div>
        </div>

        <div className="filter-group">
          <label className="filter-label">Type</label>
          <select
            className="filter-select"
            value={typeFilter}
            onChange={(e) => setTypeFilter(e.target.value as TypeFilter)}
          >
            <option value="all">All</option>
            <option value="capture">Capture</option>
            <option value="deploy">Deploy</option>
            <option value="multicast">Multicast</option>
            <option value="reboot">Reboot</option>
          </select>
        </div>

        <div className="filter-group">
          <label className="filter-label">Host</label>
          <select
            className="filter-select"
            value={hostFilter}
            onChange={(e) => setHostFilter(e.target.value)}
          >
            <option value="all">All hosts</option>
            {[...hostsById.values()].map((host) => (
              <option key={host.id.toString()} value={host.id.toString()}>
                {host.name || host.macAddress || `host ${host.id}`}
              </option>
            ))}
          </select>
        </div>

        <div className="filter-spacer" />

        <span className="filter-count">
          {filtered.length} of {tasks.length}
        </span>
      </div>

      {tasksQuery.isLoading && <div className="state">Loading…</div>}
      {tasksQuery.error && (
        <div className="state error">Failed to load tasks.</div>
      )}
      {tasksQuery.data && tasks.length === 0 && (
        <div className="state">No tasks yet.</div>
      )}
      {tasksQuery.data && tasks.length > 0 && filtered.length === 0 && (
        <div className="state">No tasks match the current filters.</div>
      )}

      {filtered.length > 0 && (
        <div className="table-card">
          <table className="table">
            <colgroup>
              <col className="col-id" />
              <col className="col-status-text" />
              <col className="col-type" />
              <col className="col-name" />
              <col className="col-captured" />
              <col className="col-captured" />
              <col className="col-actions" />
            </colgroup>
            <thead>
              <tr>
                <th>ID</th>
                <th>Status</th>
                <th>Type</th>
                <th>Image</th>
                <th>Created</th>
                <th>Updated</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((task) => (
                <TaskRow
                  key={task.id.toString()}
                  task={task}
                  hostsById={hostsById}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function TaskRow({
  task,
  hostsById,
}: {
  task: Task;
  hostsById: Map<string, Host>;
}) {
  const queryClient = useQueryClient();
  const [expanded, setExpanded] = useState(false);

  const cancelMutation = useMutation({
    mutationFn: () => dashboardClient.cancelTask({ id: task.id }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
    },
    meta: { errorTitle: "Cancel task failed" },
  });

  const retryMutation = useMutation({
    mutationFn: () => dashboardClient.retryTask({ id: task.id }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
    },
    meta: { errorTitle: "Retry task failed" },
  });

  const hostName = (hostId: bigint) => {
    const h = hostsById.get(hostId.toString());
    return h?.name || h?.macAddress || `host ${hostId.toString()}`;
  };
  const hostsMissing = task.hosts.length === 0;
  const updatedAt = latestActivity(task);

  // image_id NULL means "no image" (reboot); a set id with image_deleted means
  // the image was soft-deleted (name still resolved for display).
  const imageLabel =
    task.imageId === undefined
      ? "—"
      : (task.imageName ?? `image ${task.imageId.toString()}`) +
        (task.imageDeleted ? " (deleted)" : "");
  const canCancel = ACTIVE_STATES.has(task.state);
  const canRetry = RETRYABLE_STATES.has(task.state);
  // A soft-deleted image can't be re-run (blobs cleared); the server rejects it.
  const retryDisabled = hostsMissing || task.imageDeleted;
  const busy = cancelMutation.isPending || retryMutation.isPending;
  const hasError = task.hosts.some((h) => !!h.error);

  // Deploy and multicast write to the disk; cancelling interrupts that write
  // and can leave the disk in an inconsistent state. Capture only reads.
  const cancelWarns =
    task.type === TaskType.TYPE_DEPLOY || task.type === TaskType.TYPE_MULTICAST;

  function handleCancel() {
    if (
      cancelWarns &&
      !window.confirm(
        `Cancelling a ${typeLabel(task.type)} task interrupts the disk write ` +
          `and can leave the disk in an inconsistent state. Cancel anyway?`,
      )
    ) {
      return;
    }
    cancelMutation.mutate();
  }

  return (
    <>
      <tr
        className={`row-clickable${hasError ? " row-with-error" : ""}${
          expanded ? " row-expanded" : ""
        }`}
        onClick={() => setExpanded((v) => !v)}
      >
        <td className="cell-mono cell-id">{task.id.toString()}</td>
        <td>
          <StatusBadge state={task.state} />
        </td>
        <td>
          <TypeBadge type={task.type} />
        </td>
        <td
          className={`cell-name${task.imageDeleted ? " cell-deleted" : ""}`}
          title={imageLabel}
        >
          {imageLabel}
        </td>
        <td className="cell-captured">{formatRelative(task.createdAt)}</td>
        <td className="cell-captured">{formatRelative(updatedAt)}</td>
        <td className="cell-actions" onClick={(e) => e.stopPropagation()}>
          <div className="action-group">
            {canRetry && (
              <button
                className="ghost"
                onClick={() => retryMutation.mutate()}
                disabled={busy || retryDisabled}
                title={
                  retryDisabled
                    ? "Cannot retry: hosts or image were deleted"
                    : undefined
                }
              >
                {retryMutation.isPending ? "Retrying…" : "Retry"}
              </button>
            )}
            {canCancel && (
              <button
                className="ghost danger"
                onClick={handleCancel}
                disabled={busy}
              >
                {cancelMutation.isPending ? "Cancelling…" : "Cancel"}
              </button>
            )}
          </div>
        </td>
      </tr>
      {expanded && (
        <tr className="row-detail">
          <td />
          <td colSpan={6} className="cell-detail">
            <div className="task-hosts">
              {task.hosts.length === 0 && (
                <span className="name-empty">no hosts (deleted)</span>
              )}
              {task.hosts.map((h) => {
                const when = h.finishedAt ?? h.startedAt;
                return (
                  <div key={h.hostId.toString()} className="task-host-row">
                    <span className="task-host-name">{hostName(h.hostId)}</span>
                    <StatusBadge state={h.state} error={h.error} />
                    {h.error && (
                      <span className="task-host-error" title={h.error}>
                        {h.error}
                      </span>
                    )}
                    <span className="task-host-time">
                      {when ? formatRelative(when) : "—"}
                    </span>
                  </div>
                );
              })}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}

// Most recent per-host started/finished time, falling back to creation.
function latestActivity(task: Task) {
  let latest = task.createdAt;
  let latestMs = latest ? (timestampToDate(latest)?.getTime() ?? 0) : 0;
  for (const h of task.hosts) {
    for (const t of [h.startedAt, h.finishedAt]) {
      if (!t) continue;
      const ms = timestampToDate(t)?.getTime() ?? 0;
      if (ms >= latestMs) {
        latest = t;
        latestMs = ms;
      }
    }
  }
  return latest;
}

function StatusBadge({ state, error }: { state: TaskState; error?: string }) {
  const label = stateLabel(state);
  const tone = stateTone(state);
  const title = error ? `${label}: ${error}` : label;
  return (
    <span className={`badge badge-${tone}`} title={title}>
      {label}
    </span>
  );
}

function TypeBadge({ type }: { type: TaskType }) {
  const tone = typeTone(type);
  const label = typeLabel(type);
  return (
    <span className={`badge badge-type-${tone}`}>
      <TypeIcon type={type} />
      {label}
    </span>
  );
}

function typeLabel(type: TaskType): string {
  switch (type) {
    case TaskType.TYPE_CAPTURE:
      return "capture";
    case TaskType.TYPE_DEPLOY:
      return "deploy";
    case TaskType.TYPE_MULTICAST:
      return "multicast";
    case TaskType.TYPE_REBOOT:
      return "reboot";
    default:
      return "unknown";
  }
}

function typeTone(type: TaskType): string {
  switch (type) {
    case TaskType.TYPE_CAPTURE:
      return "capture";
    case TaskType.TYPE_DEPLOY:
      return "deploy";
    case TaskType.TYPE_MULTICAST:
      return "multicast";
    case TaskType.TYPE_REBOOT:
      return "reboot";
    default:
      return "neutral";
  }
}

function TypeIcon({ type }: { type: TaskType }) {
  const props = {
    width: 12,
    height: 12,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 2.25,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
  };
  switch (type) {
    case TaskType.TYPE_CAPTURE:
      return (
        <svg {...props} aria-hidden>
          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
          <polyline points="7 10 12 15 17 10" />
          <line x1="12" x2="12" y1="15" y2="3" />
        </svg>
      );
    case TaskType.TYPE_DEPLOY:
      return (
        <svg {...props} aria-hidden>
          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
          <polyline points="17 8 12 3 7 8" />
          <line x1="12" x2="12" y1="3" y2="15" />
        </svg>
      );
    case TaskType.TYPE_MULTICAST:
      return (
        <svg {...props} aria-hidden>
          <circle cx="12" cy="12" r="1.5" fill="currentColor" />
          <path d="M16 8a5.66 5.66 0 0 1 0 8" />
          <path d="M8 16a5.66 5.66 0 0 1 0-8" />
          <path d="M19 5a9.66 9.66 0 0 1 0 14" />
          <path d="M5 19a9.66 9.66 0 0 1 0-14" />
        </svg>
      );
    case TaskType.TYPE_REBOOT:
      return (
        <svg {...props} aria-hidden>
          <path d="M3 12a9 9 0 1 0 3-6.7" />
          <polyline points="3 3 3 8 8 8" />
        </svg>
      );
    default:
      return null;
  }
}

function stateLabel(state: TaskState): string {
  switch (state) {
    case TaskState.TASK_PENDING:
      return "pending";
    case TaskState.TASK_RUNNING:
      return "running";
    case TaskState.TASK_DONE:
      return "done";
    case TaskState.TASK_CANCELLED:
      return "cancelled";
    case TaskState.TASK_FAILED:
      return "failed";
    case TaskState.TASK_PARTIAL:
      return "partial";
    default:
      return "unknown";
  }
}

function stateTone(
  state: TaskState,
):
  | "ok"
  | "progress"
  | "error"
  | "neutral"
  | "pending"
  | "cancelled"
  | "partial" {
  switch (state) {
    case TaskState.TASK_DONE:
      return "ok";
    case TaskState.TASK_RUNNING:
      return "progress";
    case TaskState.TASK_FAILED:
      return "error";
    case TaskState.TASK_PENDING:
      return "pending";
    case TaskState.TASK_CANCELLED:
      return "cancelled";
    case TaskState.TASK_PARTIAL:
      return "partial";
    default:
      return "neutral";
  }
}

function createdMillis(task: Task): number {
  const date = timestampToDate(task.createdAt);
  return date ? date.getTime() : 0;
}
