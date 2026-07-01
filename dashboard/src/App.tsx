import { useEffect, useState } from "react";
import { Sidebar, type Tab } from "./Sidebar";
import { HostsView } from "./HostsView";
import { GroupsView } from "./GroupsView";
import { ImagesView } from "./ImagesView";
import { TasksView } from "./TasksView";
import { Toaster } from "./Toaster";
import { useConnectionStream } from "./useConnectionStream";

const VALID_TABS: Tab[] = ["hosts", "groups", "images", "tasks"];
const DEFAULT_TAB: Tab = "hosts";

function readTab(): Tab {
  const raw = window.location.hash.replace(/^#\/?/, "");
  return (VALID_TABS as string[]).includes(raw) ? (raw as Tab) : DEFAULT_TAB;
}

export default function App() {
  const [tab, setTab] = useState<Tab>(readTab);
  useConnectionStream();

  useEffect(() => {
    if (!window.location.hash) {
      window.history.replaceState(null, "", `#/${tab}`);
    }
    const onHashChange = () => setTab(readTab());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  function selectTab(next: Tab) {
    if (next === tab) return;
    setTab(next);
    window.history.pushState(null, "", `#/${next}`);
  }

  return (
    <div className="app">
      <Sidebar active={tab} onSelect={selectTab} />
      <main className="main">
        {tab === "hosts" && <HostsView />}
        {tab === "groups" && <GroupsView />}
        {tab === "images" && <ImagesView />}
        {tab === "tasks" && <TasksView />}
      </main>
      <Toaster />
    </div>
  );
}
