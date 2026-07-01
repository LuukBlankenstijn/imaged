import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Host } from "@imaged/gen/v1/dashboard/host_pb";
import { dashboardClient } from "./transport";
import { formatBytes } from "./format";
import { useConnection } from "./connectionStore";

type RowMode = "view" | "renaming" | "deploying";

export function HostsView() {
  const { data, isLoading, error } = useQuery({
    queryKey: ["hosts"],
    queryFn: () => dashboardClient.getAllHosts({}),
    refetchInterval: 4_000,
  });

  const hosts = data?.hosts ?? [];

  return (
    <>
      <header className="page-head">
        <h1 className="page-title">Hosts</h1>
        <span className="page-meta">
          <strong>{hosts.length}</strong> tracked
        </span>
      </header>

      {isLoading && <div className="state">Loading…</div>}
      {error && <div className="state error">Failed to load hosts.</div>}
      {data && hosts.length === 0 && <div className="state">No hosts yet.</div>}

      {hosts.length > 0 && (
        <div className="table-card">
          <table className="table">
            <colgroup>
              <col className="col-id" />
              <col className="col-status" />
              <col className="col-mac" />
              <col className="col-name" />
              <col className="col-disk" />
              <col className="col-actions-host" />
            </colgroup>
            <thead>
              <tr>
                <th>ID</th>
                <th></th>
                <th>MAC</th>
                <th>Name</th>
                <th className="right">Disk</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {hosts.map((host) => (
                <HostRow key={host.id.toString()} host={host} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function HostRow({ host }: { host: Host }) {
  const queryClient = useQueryClient();
  const [mode, setMode] = useState<RowMode>("view");
  const [draft, setDraft] = useState(host.name);
  const [deployImageId, setDeployImageId] = useState<string>("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (mode === "renaming") inputRef.current?.focus();
  }, [mode]);

  useEffect(() => {
    if (mode !== "renaming") setDraft(host.name);
  }, [host.name, mode]);

  const imagesQuery = useQuery({
    queryKey: ["images"],
    queryFn: () => dashboardClient.getAllImages({}),
    enabled: mode === "deploying",
  });

  const images = useMemo(
    () => imagesQuery.data?.images ?? [],
    [imagesQuery.data],
  );

  useEffect(() => {
    if (mode === "deploying" && !deployImageId && images.length > 0) {
      setDeployImageId(images[0].id.toString());
    }
  }, [mode, deployImageId, images]);

  const renameMutation = useMutation({
    mutationFn: (newName: string) =>
      dashboardClient.updateHostName({ id: host.id, newName }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["hosts"] });
      setMode("view");
    },
    meta: { errorTitle: "Rename host failed" },
  });

  const deleteMutation = useMutation({
    mutationFn: () => dashboardClient.deleteHost({ id: host.id }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["hosts"] });
    },
    meta: { errorTitle: "Delete host failed" },
  });

  const deployMutation = useMutation({
    mutationFn: (imageId: bigint) =>
      dashboardClient.deploy({ id: host.id, imageId }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
      setMode("view");
      setDeployImageId("");
    },
    meta: { errorTitle: "Deploy failed" },
  });

  const dirty = draft !== host.name;
  const busy =
    renameMutation.isPending ||
    deleteMutation.isPending ||
    deployMutation.isPending;

  function commitRename() {
    if (dirty) renameMutation.mutate(draft);
    else setMode("view");
  }

  function startDeploy() {
    setMode("deploying");
  }

  function submitDeploy() {
    if (!deployImageId) return;
    deployMutation.mutate(BigInt(deployImageId));
  }

  function cancel() {
    setDraft(host.name);
    setDeployImageId("");
    setMode("view");
  }

  function remove() {
    const label = host.name || `host ${host.id}`;
    if (window.confirm(`Delete ${label}?`)) deleteMutation.mutate();
  }

  return (
    <tr>
      <td className="cell-mono cell-id">{host.id.toString()}</td>
      <td className="cell-status"><StatusDot id={host.id} /></td>
      <td className="cell-mono cell-mac">{host.macAddress}</td>
      <td className="cell-name">
        {mode === "renaming" ? (
          <input
            ref={inputRef}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitRename();
              if (e.key === "Escape") cancel();
            }}
            disabled={busy}
            placeholder="unnamed"
          />
        ) : host.name ? (
          host.name
        ) : (
          <span className="name-empty">unnamed</span>
        )}
      </td>
      <td className="cell-disk">{formatBytes(host.diskSizeBytes)}</td>
      <td className="cell-actions">
        <div className="action-group">
          {mode === "renaming" && (
            <>
              <button className="primary" onClick={commitRename} disabled={busy || !dirty}>
                {renameMutation.isPending ? "Saving…" : "Save"}
              </button>
              <button onClick={cancel} disabled={busy}>Cancel</button>
            </>
          )}

          {mode === "deploying" && (
            <>
              <select
                className="row-select"
                value={deployImageId}
                onChange={(e) => setDeployImageId(e.target.value)}
                disabled={busy || imagesQuery.isLoading}
              >
                {imagesQuery.isLoading && <option value="">Loading…</option>}
                {!imagesQuery.isLoading && images.length === 0 && (
                  <option value="">No images</option>
                )}
                {images.map((image) => (
                  <option key={image.id.toString()} value={image.id.toString()}>
                    {image.name || `image ${image.id}`}
                  </option>
                ))}
              </select>
              <button
                className="primary"
                onClick={submitDeploy}
                disabled={busy || !deployImageId}
              >
                {deployMutation.isPending ? "Deploying…" : "Deploy"}
              </button>
              <button onClick={cancel} disabled={busy}>Cancel</button>
            </>
          )}

          {mode === "view" && (
            <>
              <button className="ghost" onClick={startDeploy} disabled={busy}>
                Deploy
              </button>
              <button className="ghost" onClick={() => setMode("renaming")} disabled={busy}>
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

function StatusDot({ id }: { id: bigint }) {
  const online = useConnection(id);
  return (
    <span
      className={`status-dot${online ? " online" : ""}`}
      title={online ? "online" : "offline"}
    />
  );
}
