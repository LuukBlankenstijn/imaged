import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Image } from "@imaged/gen/v1/dashboard/dashboard_pb";
import { dashboardClient } from "./transport";
import { formatBytes, formatRelative } from "./format";

export function ImagesView() {
  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);
  const [draftName, setDraftName] = useState("");
  const createInputRef = useRef<HTMLInputElement>(null);

  const { data, isLoading, error } = useQuery({
    queryKey: ["images"],
    queryFn: () => dashboardClient.getAllImages({}),
  });

  const createMutation = useMutation({
    mutationFn: (name: string) => dashboardClient.createImage({ name }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["images"] });
      setCreating(false);
      setDraftName("");
    },
  });

  useEffect(() => {
    if (creating) createInputRef.current?.focus();
  }, [creating]);

  const images = data?.images ?? [];

  function submitCreate() {
    const name = draftName.trim();
    if (name) createMutation.mutate(name);
  }

  function cancelCreate() {
    setCreating(false);
    setDraftName("");
  }

  return (
    <>
      <header className="page-head">
        <h1 className="page-title">Images</h1>
        <div className="head-actions">
          <span className="page-meta">
            <strong>{images.length}</strong> total
          </span>
          {!creating && (
            <button className="primary" onClick={() => setCreating(true)}>
              + New image
            </button>
          )}
        </div>
      </header>

      {creating && (
        <div className="create-bar">
          <input
            ref={createInputRef}
            placeholder="Image name…"
            value={draftName}
            onChange={(e) => setDraftName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") submitCreate();
              if (e.key === "Escape") cancelCreate();
            }}
            disabled={createMutation.isPending}
          />
          <button
            className="primary"
            onClick={submitCreate}
            disabled={createMutation.isPending || !draftName.trim()}
          >
            {createMutation.isPending ? "Creating…" : "Create"}
          </button>
          <button onClick={cancelCreate} disabled={createMutation.isPending}>
            Cancel
          </button>
        </div>
      )}

      {isLoading && <div className="state">Loading…</div>}
      {error && <div className="state error">Failed to load images.</div>}
      {data && images.length === 0 && !creating && (
        <div className="state">No images yet.</div>
      )}

      {images.length > 0 && (
        <div className="table-card">
          <table className="table">
            <colgroup>
              <col className="col-id" />
              <col className="col-status-text" />
              <col className="col-name" />
              <col className="col-captured" />
              <col className="col-parts" />
              <col className="col-disk" />
              <col className="col-actions-wide" />
            </colgroup>
            <thead>
              <tr>
                <th>ID</th>
                <th>Status</th>
                <th>Name</th>
                <th>Captured</th>
                <th className="right">Parts</th>
                <th className="right">Size</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {images.map((image) => (
                <ImageRow key={image.id.toString()} image={image} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function ImageRow({ image }: { image: Image }) {
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(image.name);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing) inputRef.current?.focus();
  }, [editing]);

  useEffect(() => {
    if (!editing) setDraft(image.name);
  }, [image.name, editing]);

  const renameMutation = useMutation({
    mutationFn: (newName: string) =>
      dashboardClient.updateImageName({ id: image.id, newName }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["images"] });
      setEditing(false);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: () => dashboardClient.deleteImage({ id: image.id }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["images"] });
    },
  });

  const dirty = draft !== image.name;
  const busy = renameMutation.isPending || deleteMutation.isPending;

  const totalSize = image.partitions.reduce(
    (sum, p) => sum + p.sizeBytes,
    0n,
  );

  function commit() {
    if (dirty) renameMutation.mutate(draft);
    else setEditing(false);
  }

  function cancel() {
    setDraft(image.name);
    setEditing(false);
  }

  function remove() {
    const label = image.name || `image ${image.id}`;
    if (window.confirm(`Delete ${label}?`)) deleteMutation.mutate();
  }

  return (
    <tr>
      <td className="cell-mono cell-id">{image.id.toString()}</td>
      <td><StatusBadge status={image.status} error={image.errorMessage} /></td>
      <td className="cell-name">
        {editing ? (
          <input
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              if (e.key === "Escape") cancel();
            }}
            disabled={busy}
            placeholder="unnamed"
          />
        ) : image.name ? (
          image.name
        ) : (
          <span className="name-empty">unnamed</span>
        )}
      </td>
      <td className="cell-captured">{formatRelative(image.capturedAt)}</td>
      <td className="cell-parts">{image.partitions.length}</td>
      <td className="cell-disk">{formatBytes(totalSize)}</td>
      <td className="cell-actions">
        <div className="action-group">
          {editing ? (
            <>
              <button className="primary" onClick={commit} disabled={busy || !dirty}>
                {renameMutation.isPending ? "Saving…" : "Save"}
              </button>
              <button onClick={cancel} disabled={busy}>Cancel</button>
            </>
          ) : (
            <>
              <button className="ghost" onClick={() => setEditing(true)} disabled={busy}>
                Rename
              </button>
              <button className="ghost danger" onClick={remove} disabled={busy}>
                {deleteMutation.isPending ? "Deleting…" : "Delete"}
              </button>
            </>
          )}
        </div>
      </td>
    </tr>
  );
}

function StatusBadge({ status, error }: { status: string; error?: string }) {
  const tone = statusTone(status);
  return (
    <span className={`badge badge-${tone}`} title={error || status}>
      {status || "—"}
    </span>
  );
}

function statusTone(status: string): "ok" | "progress" | "error" | "neutral" {
  const s = status.toLowerCase();
  if (s.includes("ready") || s.includes("complete") || s.includes("done")) return "ok";
  if (s.includes("captur") || s.includes("process") || s.includes("pending")) return "progress";
  if (s.includes("fail") || s.includes("error")) return "error";
  return "neutral";
}
