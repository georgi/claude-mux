import { createRoot } from "react-dom/client";
import "./index.css";
import { registerSw } from "./push";
import { captureInstall } from "./pwa";
import { App } from "./views/App";
import { initTheme } from "./views/settings/theme";

// Set the stored theme (`cmux.theme`, else the OS scheme) on <html> before
// the first render. No inline script: the CSP forbids it, so this runs from the bundle.
initTheme();

captureInstall();
// Production only: the dev server has no sw.js.
if (import.meta.env.PROD && "serviceWorker" in navigator) registerSw().catch(() => {});

createRoot(document.getElementById("app")!).render(<App />);
