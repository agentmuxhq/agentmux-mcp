import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { RefreshCw } from "lucide-react";
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

/** Always-visible row above Settings: current version, and a one-click update when a newer release exists. */
export function UpdateRow() {
  const [version, setVersion] = useState<string | null>(null);
  const [found, setFound] = useAvailableUpdate();
  const [checking, setChecking] = useState(false);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
  const { pct, err, install } = useInstall();

  const check = async (manual: boolean) => {
    setChecking(true);
    if (manual) setNote(null);
    try {
      const r = await api.checkUpdate();
      if (r.available) setFound({ version: r.version!, notes: r.notes });
      else if (manual) setNote({ ok: true, text: "You're on the latest version" });
    } catch (e) {
      // The automatic check stays quiet; only a click reports problems.
      if (manual) setNote({ ok: false, text: /network|dns|connect|resolve|fetch|request/i.test(String(e)) ? "Couldn't reach GitHub" : `Couldn't check: ${e}` });
    } finally {
      setChecking(false);
    }
  };

  useEffect(() => {
    api.appVersion().then((v) => setVersion(v.version)).catch(() => {});
    const t = setTimeout(() => void check(false), 3000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!note) return;
    const t = setTimeout(() => setNote(null), 5000);
    return () => clearTimeout(t);
  }, [note]);

  const installing = pct !== null;
  return (
    <div className="border-t border-white/10 px-4 py-2.5 text-xs">
      {found ? (
        <button
          onClick={install}
          disabled={installing}
          title={`Downloads the signed update for ${found.version}, installs it and restarts AgentMux. Your rooms, sessions and settings stay as they are, and a backup of your vault is saved first.${found.notes ? `\n\n${found.notes}` : ""}`}
          className="flex w-full items-center gap-2 rounded-md bg-emerald-500/15 px-3 py-2 text-left font-medium text-emerald-200 hover:bg-emerald-500/25 disabled:opacity-80"
        >
          <span className="relative flex h-2 w-2">
            <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-60" />
            <span className="relative inline-flex h-2 w-2 rounded-full bg-emerald-400" />
          </span>
          <span className="min-w-0 flex-1 truncate">
            {!installing ? `Update to v${found.version.replace(/^v/, "")}` : pct >= 99 ? "Installing, restarting…" : `Downloading… ${pct}%`}
          </span>
          {installing && (
            <span className="h-1 w-14 overflow-hidden rounded-full bg-white/10">
              <span className="block h-full bg-emerald-400 transition-all" style={{ width: `${Math.max(4, pct)}%` }} />
            </span>
          )}
        </button>
      ) : (
        <button onClick={() => void check(true)} disabled={checking} className="flex w-full items-center gap-2 text-left text-zinc-500 hover:text-zinc-300" title="Check GitHub for a newer release">
          <RefreshCw size={12} className={checking ? "animate-spin" : ""} />
          <span className="min-w-0 flex-1 truncate">{checking ? "Checking for updates…" : version ? `v${version} · Up to date` : "Check for updates"}</span>
        </button>
      )}
      {(note || err) && <div className={`mt-1.5 break-words ${err || note?.ok === false ? "text-red-300" : "text-zinc-500"}`}>{err ?? note?.text}</div>}
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
