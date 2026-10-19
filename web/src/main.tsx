import { createRoot } from "react-dom/client";
import "./index.css";
import { registerSw } from "./push";
import { captureInstall } from "./pwa";
import { App } from "./views/App";

// Dark is the default; follow the OS scheme by toggling `dark` on <html>.
// No inline script: the CSP forbids it, so this runs from the bundle.
const scheme = matchMedia("(prefers-color-scheme: light)");
const applyScheme = () => document.documentElement.classList.toggle("dark", !scheme.matches);
applyScheme();
scheme.addEventListener("change", applyScheme);

captureInstall();
// Production only: the dev server has no sw.js.
if (import.meta.env.PROD && "serviceWorker" in navigator) registerSw().catch(() => {});

createRoot(document.getElementById("app")!).render(<App />);
