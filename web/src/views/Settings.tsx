import { BellIcon, BotIcon, FolderGitIcon, GaugeIcon, MonitorSmartphoneIcon, ServerIcon, SmartphoneIcon } from "lucide-react";
import { useState } from "react";
import { type Api, endSession } from "../api/http";
import { App, Repositories } from "./settings/app";
import { Devices } from "./settings/devices";
import { Notifications } from "./settings/notifications";
import { Claude, Limits, Server, useSettings } from "./settings/server";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useIsMobile } from "@/hooks/use-mobile";

export { setTheme, toggleTheme } from "./settings/theme";

interface Props {
  api: Api;
  readOnly?: boolean;
  /** Called after this device revoked itself. Default: end the session and reload. */
  onSignedOut?: () => void;
}

const SECTIONS = [
  ["devices", "Devices", SmartphoneIcon],
  ["notifications", "Notifications", BellIcon],
  ["repositories", "Repositories", FolderGitIcon],
  ["limits", "Limits", GaugeIcon],
  ["claude", "Claude", BotIcon],
  ["app", "App", MonitorSmartphoneIcon],
  ["server", "Server", ServerIcon],
] as const;

type SectionId = (typeof SECTIONS)[number][0];

const LAST = "cmux.settings";

function lastSection(): SectionId {
  try {
    const v = localStorage.getItem(LAST);
    return SECTIONS.some(([id]) => id === v) ? (v as SectionId) : "devices";
  } catch {
    return "devices";
  }
}

async function signOut() {
  await endSession().catch(() => {});
  location.reload();
}

/** Settings (UI design 4.3): a section nav (left on desktop, tabs on a phone) and one card per section. */
export function Settings({ api, readOnly, onSignedOut = () => void signOut() }: Props) {
  const mobile = useIsMobile();
  const [tab, setTab] = useState(lastSection);
  const settings = useSettings(api);
  const pick = (v: SectionId) => {
    setTab(v);
    try {
      localStorage.setItem(LAST, v);
    } catch {
      // Private mode: the section is remembered for this page only.
    }
  };
  const body: Record<SectionId, React.ReactNode> = {
    devices: <Devices api={api} readOnly={readOnly} onSignedOut={onSignedOut} />,
    notifications: <Notifications api={api} />,
    repositories: <Repositories api={api} readOnly={readOnly} />,
    limits: <Limits settings={settings} />,
    claude: <Claude settings={settings} />,
    app: <App />,
    server: <Server api={api} />,
  };
  return (
    <section aria-labelledby="settings-title" className="mx-auto flex w-full max-w-4xl flex-col gap-4 p-4 md:p-6">
      <h2 id="settings-title" className="text-lg font-semibold tracking-tight">
        Settings
      </h2>
      <Tabs
        value={tab}
        onValueChange={(v) => pick(v as SectionId)}
        orientation={mobile ? "horizontal" : "vertical"}
        className="gap-4 md:flex-row md:gap-6"
      >
        <TabsList
          variant="line"
          aria-label="Settings sections"
          className={mobile ? "-mx-4 w-auto max-w-none justify-start overflow-x-auto px-4 [scrollbar-width:none]" : "sticky top-4 w-44 shrink-0 items-stretch"}
        >
          {SECTIONS.map(([id, label, Icon]) => (
            <TabsTrigger key={id} value={id} className={mobile ? "flex-none" : "h-8 justify-start px-2"}>
              <Icon /> {label}
            </TabsTrigger>
          ))}
        </TabsList>
        {SECTIONS.map(([id]) => (
          <TabsContent key={id} value={id} className="min-w-0">
            {body[id]}
          </TabsContent>
        ))}
      </Tabs>
    </section>
  );
}
