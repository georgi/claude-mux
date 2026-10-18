import { render } from "preact";
import "./style.css";
import { registerSw } from "./push";
import { captureInstall } from "./pwa";
import { App } from "./views/App";

captureInstall();
// Production only: the dev server has no sw.js.
if (import.meta.env.PROD && "serviceWorker" in navigator) registerSw().catch(() => {});

render(<App />, document.getElementById("app")!);
