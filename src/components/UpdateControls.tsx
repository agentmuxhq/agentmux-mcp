import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../lib/api";
import { btn, btnPrimary } from "./ui";

type Found = { version: string; notes?: string | null };

/** Shared by the sidebar banner and Settings: listens for the background check's result. */
export function useAvailableUpdate() {
  const [found, setFound] = useState<Found | null>(null);
  useEffect(() => {
    const off = listen<Found>("update://available", (e) => setFound(e.payload));
    return () => void off.then((f) => f());
  }, []);
  return [found, setFound] as const;
}

/** Installs the update, showing download progress; the app relaunches by itself when done. */
export function useInstall() {
  const [pct, setPct] = useState<number | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    const off = listen<{ done: number; total?: number | null }>("update://progress", (e) => {
      const { done, total } = e.payload;
      setPct(total ? Math.min(99, Math.round((done / total) * 100)) : 0);
    });
    return () => void off.then((f) => f());
  }, []);
  const install = async () => {
    setErr(null);
    setPct(0);
    try {
      await api.installUpdate();
    } catch (e) {
      setErr(String(e));
      setPct(null);
    }
  };
  return { pct, err, install };
}

/** Sidebar footer: appears only when a newer version exists. */
export function UpdateBanner() {
  const [found] = useAvailableUpdate();
  const { pct, err, install } = useInstall();
  if (!found) return null;
  return (
    <div className="border-t border-white/10 bg-emerald-500/10 px-4 py-2.5 text-xs">
      <div className="flex items-center gap-2">
        <span className="min-w-0 flex-1 text-emerald-200">AgentMux {found.version} is available</span>
        <button className={btnPrimary} disabled={pct !== null} onClick={install}>
          {pct === null ? "Update & restart" : pct >= 99 ? "Installing…" : `Downloading ${pct}%`}
        </button>
      </div>
      {err && <div className="mt-1.5 break-words text-red-300">{err}</div>}
    </div>
  );
}

/** Settings → Daemon: version, manual check, and the switch for the automatic check. */
export function UpdateSettings() {
  const [info, setInfo] = useState<{ version: string; autoCheck: boolean } | null>(null);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [found, setFound] = useAvailableUpdate();
  const { pct, err, install } = useInstall();

  useEffect(() => {
    api.appVersion().then(setInfo).catch(() => {});
  }, []);

  const check = async () => {
    setBusy(true);
    setMsg(null);
    try {
      const r = await api.checkUpdate();
      if (r.available) setFound({ version: r.version!, notes: r.notes });
      else setMsg({ ok: true, text: "You're on the latest version." });
    } catch (e) {
      setMsg({ ok: false, text: `Couldn't check for updates: ${e}` });
    } finally {
      setBusy(false);
    }
  };

  if (!info) return null;
  return (
    <div className="mt-4 space-y-2 rounded-lg border border-white/10 p-3 text-xs">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-zinc-500">Version</span>
        <span className="font-mono text-zinc-200">{info.version}</span>
        <button className={`${btn} ml-auto`} disabled={busy || pct !== null} onClick={check}>
          Check for updates
        </button>
        {found && (
          <button className={btnPrimary} disabled={pct !== null} onClick={install}>
            {pct === null ? `Install ${found.version} & restart` : pct >= 99 ? "Installing…" : `Downloading ${pct}%`}
          </button>
        )}
      </div>
      <label className="flex cursor-pointer items-start gap-2 text-zinc-300">
        <input
          type="checkbox"
          className="mt-0.5"
          checked={info.autoCheck}
          onChange={(e) => api.setAutoUpdate(e.target.checked).then(() => setInfo({ ...info, autoCheck: e.target.checked }))}
        />
        <span>
          Check for updates automatically
          <span className="mt-0.5 block text-[11px] text-zinc-600">
            Contacts github.com/agentmuxhq/agentmux-mcp/releases on launch and every 6 hours. It is the only call AgentMux makes on its own.
          </span>
        </span>
      </label>
      {found?.notes && <pre className="max-h-28 overflow-auto whitespace-pre-wrap rounded bg-black/40 p-2 text-[11px] text-zinc-400">{found.notes}</pre>}
      {msg && <div className={msg.ok ? "text-emerald-300" : "text-red-300"}>{msg.text}</div>}
      {err && <div className="break-words text-red-300">{err}</div>}
    </div>
  );
}
