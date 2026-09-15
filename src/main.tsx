import React from "react";
import ReactDOM from "react-dom/client";
import { error as logError } from "@tauri-apps/plugin-log";
import App from "./App";
import "@/lib/i18n";
import { bootstrapAppearance } from "@/lib/appearance";
import { isTauri } from "@/lib/runtime";
import "./index.css";

// Forward uncaught webview errors into the same rotating log file as the Rust
// side, so a "Report a problem" bundle captures frontend crashes too.
if (isTauri()) {
  window.addEventListener("error", (e) => {
    void logError(`[webview] ${e.message} @ ${e.filename}:${e.lineno}`);
  });
  window.addEventListener("unhandledrejection", (e) => {
    void logError(`[webview] unhandled rejection: ${String(e.reason)}`);
  });
}

bootstrapAppearance();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
