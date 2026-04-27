import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type {
  Host,
  Image,
  Task,
} from "@imaged/gen/v1/dashboard/dashboard_pb";
import { TaskState, TaskType } from "@imaged/gen/v1/dashboard/dashboard_pb";
import { dashboardClient } from "./transport";
import { formatRelative, timestampToDate } from "./format";

type TypeFilter = "all" | "capture" | "deploy";
type StatusFilter = "active" | "all" | "completed";

const ACTIVE_STATES = new Set<TaskState>([
  TaskState.TASK_PENDING,
  TaskState.TASK_RUNNING,
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

  const imagesQuery = useQuery({
    queryKey: ["images"],
    queryFn: () => dashboardClient.getAllImages({}),
  });

  const hostsById = useMemo(() => {
    const map = new Map<string, Host>();
    for (const h of hostsQuery.data?.hosts ?? []) map.set(h.id.toString(), h);
    return map;
  }, [hostsQuery.data]);

  const imagesById = useMemo(() => {
    const map = new Map<string, Image>();
    for (const i of imagesQuery.data?.images ?? []) map.set(i.id.toString(), i);
    return map;
  }, [imagesQuery.data]);

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
        const active = ACTIVE_STATES.has(t.state);
        if (statusFilter === "active" && !active) return false;
        if (statusFilter === "completed" && active) return false;
        if (hostFilter !== "all") {
          if (t.hostId === undefined) return false;
          if (t.hostId.toString() !== hostFilter) return false;
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
                <th>Host</th>
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
                  host={
                    task.hostId !== undefined
                      ? hostsById.get(task.hostId.toString())
                      : undefined
                  }
                  image={
                    task.imageId !== undefined
                      ? imagesById.get(task.imageId.toString())
                      : undefined
                  }
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
  host,
  image,
}: {
  task: Task;
  host?: Host;
  image?: Image;
}) {
  const queryClient = useQueryClient();

  const cancelMutation = useMutation({
    mutationFn: () => dashboardClient.cancelTask({ id: task.id }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
    },
    meta: { errorTitle: "Cancel task failed" },
  });

  const updatedAt = task.finishedAt ?? task.startedAt ?? task.createdAt;
  const hostMissing = task.hostId === undefined;
  const imageMissing = task.imageId === undefined;
  const hostLabel = hostMissing
    ? "(deleted)"
    : host?.name || host?.macAddress || `host ${task.hostId!.toString()}`;
  const imageLabel = imageMissing
    ? "(deleted)"
    : image?.name || `image ${task.imageId!.toString()}`;
  const canCancel = ACTIVE_STATES.has(task.state);
  const showError = !!task.error;

  return (
    <>
      <tr className={showError ? "row-with-error" : undefined}>
        <td className="cell-mono cell-id">{task.id.toString()}</td>
        <td>
          <StatusBadge state={task.state} error={task.error} />
        </td>
        <td>
          <TypeBadge type={task.type} />
        </td>
        <td
          className={`cell-name${hostMissing ? " cell-deleted" : ""}`}
          title={host?.macAddress}
        >
          {hostLabel}
        </td>
        <td
          className={`cell-name${imageMissing ? " cell-deleted" : ""}`}
          title={imageLabel}
        >
          {imageLabel}
        </td>
        <td className="cell-captured">{formatRelative(task.createdAt)}</td>
        <td className="cell-captured">{formatRelative(updatedAt)}</td>
        <td className="cell-actions">
          <div className="action-group">
            {canCancel && (
              <button
                className="ghost danger"
                onClick={() => cancelMutation.mutate()}
                disabled={cancelMutation.isPending}
              >
                {cancelMutation.isPending ? "Cancelling…" : "Cancel"}
              </button>
            )}
          </div>
        </td>
      </tr>
      {showError && (
        <tr className="row-error">
          <td />
          <td colSpan={7} className="cell-error">
            <div className="error-callout">{task.error}</div>
          </td>
        </tr>
      )}
    </>
  );
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
  const label =
    type === TaskType.TYPE_CAPTURE
      ? "capture"
      : type === TaskType.TYPE_DEPLOY
        ? "deploy"
        : "unknown";
  return <span className={`badge badge-neutral`}>{label}</span>;
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
    default:
      return "unknown";
  }
}

function stateTone(
  state: TaskState,
): "ok" | "progress" | "error" | "neutral" | "pending" | "cancelled" {
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
    default:
      return "neutral";
  }
}

function createdMillis(task: Task): number {
  const date = timestampToDate(task.createdAt);
  return date ? date.getTime() : 0;
}
