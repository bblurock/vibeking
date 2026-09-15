import { useEffect } from "react";
import {
  createMemoryRouter,
  RouterProvider,
  Outlet,
} from "react-router-dom";
import { Sidebar } from "@/components/Sidebar";
import { Home } from "@/routes/Home";
import { Hotwords } from "@/routes/Hotwords";
import { History } from "@/routes/History";
import { Corrections } from "@/routes/Corrections";
import { Support } from "@/routes/Support";
import { ChipBar, ModePickerWindow } from "@/routes/ChipBar";
import { startHistoryListener } from "@/lib/history";
import { startSoundListener } from "@/lib/sound";
import { startTrayListener } from "@/lib/tray";
import { ErrorBanner } from "@/components/ErrorBanner";
import { ParakeetPrepBanner } from "@/components/ParakeetPrepBanner";
import { PermissionsGate } from "@/components/PermissionsGate";

function Shell() {
  useEffect(() => {
    startHistoryListener();
    startSoundListener();
    startTrayListener();
  }, []);

  return (
    <PermissionsGate>
      <div className="flex h-screen w-screen overflow-hidden">
        <Sidebar />
        <main className="flex-1 overflow-y-auto">
          <Outlet />
        </main>
        <ErrorBanner />
        <ParakeetPrepBanner />
      </div>
    </PermissionsGate>
  );
}

const router = createMemoryRouter([
  {
    path: "/",
    element: <Shell />,
    children: [
      { index: true, element: <Home /> },
      { path: "hotwords", element: <Hotwords /> },
      { path: "history", element: <History /> },
      { path: "corrections", element: <Corrections /> },
      { path: "support", element: <Support /> },
    ],
  },
]);

function App() {
  const params = new URLSearchParams(window.location.search);
  const which = params.get("window");
  if (which === "chipbar") return <ChipBar />;
  if (which === "modepicker") return <ModePickerWindow />;
  return <RouterProvider router={router} />;
}

export default App;
