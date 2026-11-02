import { useEffect, useState } from "react";
import { Check, Copy } from "lucide-react";
import { api, ago, type SyncReport, type SyncStatus } from "../lib/api";
import { Badge, btn, btnPrimary } from "./ui";

const input =
  "min-w-0 flex-1 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1.5 font-mono text-[11px] text-zinc-200 outline-none focus:border-white/30 disabled:opacity-60";

/** Settings → Sync: back up rooms and setup to your own private git repo, and restore them on another Mac. */
export default function SyncPanel() {
  const [st, setSt] = useState<SyncStatus | null>(null);
  const [remote, setRemote] = useState("");
  const [diffs, setDiffs] = useState(false);
  const [auto, setAuto] = useState(true);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const load = () =>
    api
      .syncStatus()
      .then((s) => {
        setSt(s);
        setDiffs(s.includeDiffs);
        setAuto(s.configured ? s.auto : true);
      })
      .catch((e) => setErr(String(e)));
  useEffect(() => void load(), []);

  const summary = (r: SyncReport) => {
    const got = r.imported.messages + r.imported.sessions + r.imported.rooms + r.imported.bots;
    const parts = [got > 0 ? `Brought in ${r.imported.messages} messages, ${r.imported.sessions} sessions, ${r.imported.rooms} rooms${r.imported.bots ? ` and ${r.imported.bots} bots` : ""}.` : "Nothing new from other Macs.", r.pushed ? "Backed up your latest changes." : "Already up to date."];
    return parts.join(" ");
  };

  const run = async (fn: () => Promise<{ status: SyncStatus; report: SyncReport }>) => {
    setBusy(true);
    setErr(null);
    setNote(null);
    try {
      const r = await fn();
      setSt(r.status);
      setNote(summary(r.report));
    } catch (e) {
      setErr(String(e));
      load();
    } finally {
      setBusy(false);
    }
  };

  if (!st) return <div className="text-xs text-zinc-500">{err ?? "Loading…"}</div>;

  return (
    <div className="space-y-4 text-xs">
      <div className="text-zinc-400">
        Back up your rooms, sessions and setup to a <b className="text-zinc-200">private git repo you own</b> (GitHub, GitLab or any git remote), and restore them on another Mac. It's free, and AgentMux never sees your data: it uses your own git login.
      </div>

      {!st.gitInstalled && <div className="rounded-md bg-amber-500/10 p-2 text-amber-200">git isn't installed. Run <span className="font-mono">xcode-select --install</span> in a terminal first.</div>}

      {!st.configured ? (
        <div className="space-y-3 rounded-lg border border-white/10 p-3">
          <div className="font-semibold text-zinc-200">Connect a private repository</div>
          <div className="text-zinc-500">Create an empty <b>private</b> repo first (for example <span className="font-mono">agentmux-backup</span>), then paste its address. Use an address your Mac can already push to.</div>
          <input value={remote} onChange={(e) => setRemote(e.target.value)} placeholder="git@github.com:you/agentmux-backup.git" spellCheck={false} className={`${input} w-full`} />
          <label className="flex cursor-pointer items-start gap-2 text-zinc-300">
            <input type="checkbox" className="mt-0.5" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
            <span>Sync automatically every 15 minutes</span>
          </label>
          <label className="flex cursor-pointer items-start gap-2 text-zinc-300">
            <input type="checkbox" className="mt-0.5" checked={diffs} onChange={(e) => setDiffs(e.target.checked)} />
            <span>
              Include git diffs and terminal output
              <span className="mt-0.5 block text-[11px] text-zinc-600">Off by default: working-tree diffs can contain secrets from your code.</span>
            </span>
          </label>
          <button className={btnPrimary} disabled={busy || !remote.trim() || !st.gitInstalled} onClick={() => run(() => api.syncConnect(remote.trim(), diffs, auto))}>
            {busy ? "Syncing…" : "Connect and sync"}
          </button>
        </div>
      ) : (
        <div className="space-y-3 rounded-lg border border-white/10 p-3">
          <div className="flex flex-wrap items-center gap-2">
            <Badge tone={st.error ? "red" : "green"}>{st.error ? "last sync failed" : "connected"}</Badge>
            {st.visibility === "private" && <Badge tone="green">repo is private</Badge>}
            {st.visibility === "unverified" && <Badge tone="amber">couldn't verify it's private</Badge>}
            <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-zinc-300" title={st.remote ?? ""}>
              {st.remote}
            </span>
            <button
              title="Copy"
              className="shrink-0 text-zinc-500 hover:text-white"
              onClick={() =>
                st.remote &&
                navigator.clipboard?.writeText(st.remote).then(() => {
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1500);
                })
              }
            >
              {copied ? <Check size={13} /> : <Copy size={13} />}
            </button>
          </div>
          <div className="text-zinc-500">{st.last ? `Last synced ${ago(st.last)}.` : "Not synced yet."}</div>
          {st.visibility === "unverified" && <div className="text-amber-300/80">AgentMux could only check GitHub and GitLab repos. Make sure this one is private.</div>}
          <label className="flex cursor-pointer items-start gap-2 text-zinc-300">
            <input
              type="checkbox"
              className="mt-0.5"
              checked={auto}
              disabled={busy}
              onChange={(e) => {
                setAuto(e.target.checked);
                api.syncSetOptions(diffs, e.target.checked).then(setSt);
              }}
            />
            <span>Sync automatically every 15 minutes</span>
          </label>
          <label className="flex cursor-pointer items-start gap-2 text-zinc-300">
            <input
              type="checkbox"
              className="mt-0.5"
              checked={diffs}
              disabled={busy}
              onChange={(e) => {
                setDiffs(e.target.checked);
                api.syncSetOptions(e.target.checked, auto).then(setSt);
              }}
            />
            <span>
              Include git diffs and terminal output
              <span className="mt-0.5 block text-[11px] text-zinc-600">Off by default: working-tree diffs can contain secrets from your code.</span>
            </span>
          </label>
          <div className="flex flex-wrap gap-2">
            <button className={btnPrimary} disabled={busy} onClick={() => run(api.syncNow)}>
              {busy ? "Syncing…" : "Sync now"}
            </button>
            <button
              className={btn}
              disabled={busy}
              onClick={async () => {
                if (!window.confirm("Stop syncing? Your data stays on this Mac and in the repo; AgentMux just forgets the repo address.")) return;
                setSt(await api.syncDisconnect());
                setNote(null);
              }}
            >
              Disconnect
            </button>
          </div>
        </div>
      )}

      {note && <div className="text-emerald-300">{note}</div>}
      {(err || st.error) && <pre className="whitespace-pre-wrap rounded-md bg-red-500/10 p-2 font-mono text-[11px] text-red-200">{err ?? st.error}</pre>}

      <div className="space-y-1.5 rounded-lg border border-white/10 p-3 text-zinc-500">
        <div className="font-semibold text-zinc-300">What goes in the repo</div>
        <div>
          Rooms, sessions, messages and tickets, your agents and bots (names and descriptions), and non-secret settings such as your tunnel hostname. Readable transcripts as Markdown too.
        </div>
        <div className="font-semibold text-zinc-300">What never does</div>
        <div>Bot tokens, webhook URLs and keys, tunnel credentials and your Keychain. Obvious credentials in message text (API keys, tokens, private keys) are replaced with [redacted], as a best effort.</div>
        <div className="font-semibold text-zinc-300">On a new Mac</div>
        <div>Install AgentMux, open Settings → Sync, enter the same repo and press Connect and sync. Rooms and history come back; reconnect your bots' tokens and webhooks.</div>
        <div>Syncing only adds to this Mac and never deletes or overwrites anything here.</div>
      </div>
    </div>
  );
}
