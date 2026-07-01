import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Group } from "@imaged/gen/v1/dashboard/group_pb";
import type { Host } from "@imaged/gen/v1/dashboard/host_pb";
import { dashboardClient } from "./transport";

export function GroupsView() {
  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);
  const [expandedId, setExpandedId] = useState<string | null>(null);

  const groupsQuery = useQuery({
    queryKey: ["groups"],
    queryFn: () => dashboardClient.getAllGroups({}),
    refetchInterval: 5_000,
  });

  const groups = groupsQuery.data?.groups ?? [];

  const createMutation = useMutation({
    mutationFn: (input: { name: string; hostIds: bigint[] }) =>
      dashboardClient.createGroup(input),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["groups"] });
      setCreating(false);
    },
    meta: { errorTitle: "Create group failed" },
  });

  return (
    <>
      <header className="page-head">
        <h1 className="page-title">Groups</h1>
        <div className="head-actions">
          <span className="page-meta">
            <strong>{groups.length}</strong> total
          </span>
          {!creating && (
            <button className="primary" onClick={() => setCreating(true)}>
              + New group
            </button>
          )}
        </div>
      </header>

      {creating && (
        <CreateGroupForm
          submitting={createMutation.isPending}
          onSubmit={(name, hostIds) => createMutation.mutate({ name, hostIds })}
          onCancel={() => setCreating(false)}
        />
      )}

      {groupsQuery.isLoading && <div className="state">Loading…</div>}
      {groupsQuery.error && (
        <div className="state error">Failed to load groups.</div>
      )}
      {groupsQuery.data && groups.length === 0 && !creating && (
        <div className="state">No groups yet.</div>
      )}

      {groups.length > 0 && (
        <div className="table-card">
          <table className="table">
            <colgroup>
              <col className="col-id" />
              <col className="col-name" />
              <col className="col-actions-host" />
            </colgroup>
            <thead>
              <tr>
                <th>ID</th>
                <th>Name</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {groups.map((group) => {
                const id = group.id.toString();
                return (
                  <GroupRow
                    key={id}
                    group={group}
                    expanded={expandedId === id}
                    onToggleExpand={() =>
                      setExpandedId(expandedId === id ? null : id)
                    }
                  />
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

function CreateGroupForm({
  submitting,
  onSubmit,
  onCancel,
}: {
  submitting: boolean;
  onSubmit: (name: string, hostIds: bigint[]) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const inputRef = useRef<HTMLInputElement>(null);

  const hostsQuery = useQuery({
    queryKey: ["hosts"],
    queryFn: () => dashboardClient.getAllHosts({}),
  });

  const hosts = hostsQuery.data?.hosts ?? [];

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  function submit() {
    const trimmed = name.trim();
    if (!trimmed) return;
    onSubmit(
      trimmed,
      [...selected].map((id) => BigInt(id)),
    );
  }

  const canSubmit = !submitting && name.trim().length > 0;

  return (
    <div className="group-create">
      <div className="create-bar">
        <input
          ref={inputRef}
          placeholder="Group name…"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
            if (e.key === "Escape") onCancel();
          }}
          disabled={submitting}
        />
        <button className="primary" onClick={submit} disabled={!canSubmit}>
          {submitting ? "Creating…" : "Create"}
        </button>
        <button onClick={onCancel} disabled={submitting}>
          Cancel
        </button>
      </div>
      <div className="host-picker">
        <div className="host-picker-label">
          Members
          <span className="host-picker-count">
            {selected.size} selected
          </span>
        </div>
        <HostChipPicker
          hosts={hosts}
          loading={hostsQuery.isLoading}
          selected={selected}
          onChange={setSelected}
          disabled={submitting}
        />
      </div>
    </div>
  );
}

function GroupRow({
  group,
  expanded,
  onToggleExpand,
}: {
  group: Group;
  expanded: boolean;
  onToggleExpand: () => void;
}) {
  const queryClient = useQueryClient();
  const [renaming, setRenaming] = useState(false);
  const [draftName, setDraftName] = useState(group.name);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (renaming) inputRef.current?.focus();
  }, [renaming]);

  useEffect(() => {
    if (!renaming) setDraftName(group.name);
  }, [group.name, renaming]);

  const renameMutation = useMutation({
    mutationFn: (newName: string) =>
      dashboardClient.updateGroupName({ id: group.id, newName }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["groups"] });
      setRenaming(false);
    },
    meta: { errorTitle: "Rename group failed" },
  });

  const deleteMutation = useMutation({
    mutationFn: () => dashboardClient.deleteGroup({ id: group.id }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["groups"] });
    },
    meta: { errorTitle: "Delete group failed" },
  });

  const dirty = draftName !== group.name;
  const busy = renameMutation.isPending || deleteMutation.isPending;

  function commitRename() {
    if (dirty) renameMutation.mutate(draftName);
    else setRenaming(false);
  }

  function cancelRename() {
    setDraftName(group.name);
    setRenaming(false);
  }

  function remove() {
    const label = group.name || `group ${group.id}`;
    if (window.confirm(`Delete ${label}?`)) deleteMutation.mutate();
  }

  return (
    <>
      <tr className={expanded ? "row-expanded" : undefined}>
        <td className="cell-mono cell-id">{group.id.toString()}</td>
        <td className="cell-name">
          {renaming ? (
            <input
              ref={inputRef}
              value={draftName}
              onChange={(e) => setDraftName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitRename();
                if (e.key === "Escape") cancelRename();
              }}
              disabled={busy}
              placeholder="unnamed"
            />
          ) : group.name ? (
            group.name
          ) : (
            <span className="name-empty">unnamed</span>
          )}
        </td>
        <td className="cell-actions">
          <div className="action-group">
            {renaming ? (
              <>
                <button
                  className="primary"
                  onClick={commitRename}
                  disabled={busy || !dirty}
                >
                  {renameMutation.isPending ? "Saving…" : "Save"}
                </button>
                <button onClick={cancelRename} disabled={busy}>
                  Cancel
                </button>
              </>
            ) : (
              <>
                <button
                  className="ghost"
                  onClick={onToggleExpand}
                  disabled={busy}
                >
                  {expanded ? "Hide" : "View"}
                </button>
                <button
                  className="ghost"
                  onClick={() => setRenaming(true)}
                  disabled={busy}
                >
                  Rename
                </button>
                <button
                  className="ghost danger"
                  onClick={remove}
                  disabled={busy}
                >
                  {deleteMutation.isPending ? "Deleting…" : "Delete"}
                </button>
              </>
            )}
          </div>
        </td>
      </tr>
      {expanded && !renaming && (
        <tr className="row-detail">
          <td />
          <td colSpan={2} className="cell-detail">
            <GroupDetail group={group} />
          </td>
        </tr>
      )}
    </>
  );
}

function GroupDetail({ group }: { group: Group }) {
  const queryClient = useQueryClient();
  const [mode, setMode] = useState<"view" | "editing" | "multicasting">("view");
  const [editSelected, setEditSelected] = useState<Set<string>>(new Set());
  const [multicastImageId, setMulticastImageId] = useState<string>("");

  const membersQuery = useQuery({
    queryKey: ["hosts", "group", group.id.toString()],
    queryFn: () => dashboardClient.getAllHosts({ groupId: group.id }),
  });
  const members = useMemo(
    () => membersQuery.data?.hosts ?? [],
    [membersQuery.data],
  );

  const allHostsQuery = useQuery({
    queryKey: ["hosts"],
    queryFn: () => dashboardClient.getAllHosts({}),
    enabled: mode === "editing",
  });
  const allHosts = allHostsQuery.data?.hosts ?? [];

  const imagesQuery = useQuery({
    queryKey: ["images"],
    queryFn: () => dashboardClient.getAllImages({}),
  });
  const images = imagesQuery.data?.images ?? [];

  useEffect(() => {
    if (mode === "multicasting" && !multicastImageId && images.length > 0) {
      setMulticastImageId(images[0].id.toString());
    }
  }, [mode, multicastImageId, images]);

  const updateMutation = useMutation({
    mutationFn: (hostIds: bigint[]) =>
      dashboardClient.updateGroupMemberships({ id: group.id, hostIds }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["hosts", "group", group.id.toString()],
      });
      setMode("view");
    },
    meta: { errorTitle: "Update members failed" },
  });

  const multicastMutation = useMutation({
    mutationFn: (imageId: bigint) =>
      dashboardClient.multicast({
        hostIds: members.map((h) => h.id),
        imageId,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
      setMode("view");
      setMulticastImageId("");
    },
    meta: { errorTitle: "Multicast failed" },
  });

  function startEdit() {
    setEditSelected(new Set(members.map((h) => h.id.toString())));
    setMode("editing");
  }

  function toggleSelected(id: string) {
    setEditSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function submitEdit() {
    updateMutation.mutate([...editSelected].map((id) => BigInt(id)));
  }

  function submitMulticast() {
    if (!multicastImageId) return;
    multicastMutation.mutate(BigInt(multicastImageId));
  }

  function cancel() {
    setMode("view");
    setMulticastImageId("");
  }

  const busy = updateMutation.isPending || multicastMutation.isPending;
  const canMulticast = members.length > 0 && images.length > 0;

  if (mode === "editing") {
    return (
      <div className="group-detail">
        <div className="group-detail-head">
          <span className="group-detail-title">
            Members
            <span className="group-detail-count">
              {editSelected.size} selected
            </span>
          </span>
          <div className="action-group">
            <button
              className="primary"
              onClick={submitEdit}
              disabled={busy}
            >
              {updateMutation.isPending ? "Saving…" : "Save"}
            </button>
            <button onClick={cancel} disabled={busy}>
              Cancel
            </button>
          </div>
        </div>
        {allHostsQuery.isLoading && (
          <div className="state">Loading hosts…</div>
        )}
        {!allHostsQuery.isLoading && allHosts.length === 0 && (
          <div className="state">No hosts available.</div>
        )}
        {allHosts.length > 0 && (
          <div className="host-chip-grid">
            {allHosts.map((host) => {
              const id = host.id.toString();
              const active = editSelected.has(id);
              const labelText =
                host.name || host.macAddress || `host ${id}`;
              return (
                <button
                  key={id}
                  className={`host-chip${active ? " active" : ""}`}
                  onClick={() => toggleSelected(id)}
                  disabled={busy}
                  type="button"
                >
                  {labelText}
                </button>
              );
            })}
          </div>
        )}
      </div>
    );
  }

  if (mode === "multicasting") {
    return (
      <div className="group-detail">
        <div className="group-detail-head">
          <span className="group-detail-title">
            Multicast
            <span className="group-detail-count">
              {members.length} {members.length === 1 ? "host" : "hosts"}
            </span>
          </span>
          <div className="action-group">
            <select
              className="row-select"
              value={multicastImageId}
              onChange={(e) => setMulticastImageId(e.target.value)}
              disabled={busy || imagesQuery.isLoading}
            >
              {imagesQuery.isLoading && <option value="">Loading…</option>}
              {!imagesQuery.isLoading && images.length === 0 && (
                <option value="">No images</option>
              )}
              {images.map((image) => (
                <option
                  key={image.id.toString()}
                  value={image.id.toString()}
                >
                  {image.name || `image ${image.id}`}
                </option>
              ))}
            </select>
            <button
              className="primary"
              onClick={submitMulticast}
              disabled={busy || !multicastImageId}
            >
              {multicastMutation.isPending ? "Sending…" : "Send"}
            </button>
            <button onClick={cancel} disabled={busy}>
              Cancel
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="group-detail">
      <div className="group-detail-head">
        <span className="group-detail-title">
          Members
          <span className="group-detail-count">{members.length}</span>
        </span>
        <div className="action-group">
          <button
            className="ghost"
            onClick={() => setMode("multicasting")}
            disabled={busy || !canMulticast}
            title={
              !canMulticast
                ? members.length === 0
                  ? "Group has no members"
                  : "No images available"
                : undefined
            }
          >
            Multicast
          </button>
          <button className="ghost" onClick={startEdit} disabled={busy}>
            Edit members
          </button>
        </div>
      </div>
      {membersQuery.isLoading && <div className="state">Loading members…</div>}
      {membersQuery.error && (
        <div className="state error">Failed to load members.</div>
      )}
      {membersQuery.data && members.length === 0 && (
        <div className="state">No members.</div>
      )}
      {members.length > 0 && (
        <div className="member-chip-grid">
          {members.map((host) => (
            <span key={host.id.toString()} className="member-chip">
              {host.name || host.macAddress || `host ${host.id}`}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function HostChipPicker({
  hosts,
  loading,
  selected,
  onChange,
  disabled,
}: {
  hosts: Host[];
  loading: boolean;
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  disabled?: boolean;
}) {
  const [search, setSearch] = useState("");

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return hosts;
    return hosts.filter(
      (h) =>
        h.name.toLowerCase().includes(q) ||
        h.macAddress.toLowerCase().includes(q),
    );
  }, [hosts, search]);

  const visibleSelectedCount = filtered.reduce(
    (n, h) => (selected.has(h.id.toString()) ? n + 1 : n),
    0,
  );

  function toggle(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onChange(next);
  }

  function selectAllVisible() {
    const next = new Set(selected);
    for (const h of filtered) next.add(h.id.toString());
    onChange(next);
  }

  function clearVisible() {
    const next = new Set(selected);
    for (const h of filtered) next.delete(h.id.toString());
    onChange(next);
  }

  const allVisibleSelected =
    filtered.length > 0 && visibleSelectedCount === filtered.length;

  return (
    <div className="chip-picker">
      <div className="chip-picker-toolbar">
        <input
          className="chip-picker-search"
          type="search"
          placeholder="Search hosts…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          disabled={disabled}
        />
        <button
          type="button"
          className="ghost"
          onClick={selectAllVisible}
          disabled={disabled || filtered.length === 0 || allVisibleSelected}
        >
          {search ? "Select matching" : "Select all"}
        </button>
        <button
          type="button"
          className="ghost"
          onClick={clearVisible}
          disabled={disabled || visibleSelectedCount === 0}
        >
          Clear
        </button>
      </div>
      {loading && <div className="state">Loading hosts…</div>}
      {!loading && hosts.length === 0 && (
        <div className="state">No hosts available.</div>
      )}
      {!loading && hosts.length > 0 && filtered.length === 0 && (
        <div className="state">No hosts match.</div>
      )}
      {filtered.length > 0 && (
        <div className="host-chip-grid">
          {filtered.map((host) => {
            const id = host.id.toString();
            const active = selected.has(id);
            const labelText = host.name || host.macAddress || `host ${id}`;
            return (
              <button
                key={id}
                className={`host-chip${active ? " active" : ""}`}
                onClick={() => toggle(id)}
                disabled={disabled}
                type="button"
              >
                {labelText}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
