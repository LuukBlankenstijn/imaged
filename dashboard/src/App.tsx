import { useState } from "react";
import { Sidebar, type Tab } from "./Sidebar";
import { HostsView } from "./HostsView";
import { ImagesView } from "./ImagesView";
import { useConnectionStream } from "./useConnectionStream";

export default function App() {
  const [tab, setTab] = useState<Tab>("hosts");
  useConnectionStream();

  return (
    <div className="app">
      <Sidebar active={tab} onSelect={setTab} />
      <main className="main">
        {tab === "hosts" && <HostsView />}
        {tab === "images" && <ImagesView />}
      </main>
    </div>
  );
}
