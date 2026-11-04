import { useEffect, useState } from "react";
import { api, getCatalog, type EngineRole, type EnginesStatus } from "../lib/api";
import { Badge, btn, btnPrimary } from "./ui";

const ROLES: { id: "writer" | "embed" | "decide" | "guard"; title: string; what: string; kinds: [string, string][] }[] = [
  {
    id: "writer",
    title: "Writer",
    what: "Writes the contextual summary of each thread and each product's brief, and answers questions from memory. Off: AgentMux builds a structured outline instead.",
    kinds: [["off", "Off (outline)"], ["ollama", "Local (Ollama)"], ["openai", "Cloud key (OpenRouter, Gemini…)"], ["agent", "One of my agents"]],
  },
  {
    id: "embed",
    title: "Search by meaning",
    what: "Finds threads by meaning, not just words (\"sprite pipeline\" finds the animation handoff). Off: search by words, thread names and summaries.",
    kinds: [["off", "Off (words)"], ["ollama", "Local (Ollama)"], ["openai", "Cloud key"]],
  },
  {
    id: "decide",
    title: "Decisions",
    what: "When a topic doesn't name a thread exactly, picks the thread (or a new one) with a confidence. Unsure means it asks you in your chat.",
    kinds: [["builtin", "Built in (rules + search)"], ["llm", "Writer engine"], ["jeff", "Jeff (local, calibrated)"]],
  },
  {
    id: "guard",
    title: "Guard",
    what: "Screens what cloud bots (Grok) send before your local agents read it, and blocks likely prompt injections.",
    kinds: [["off", "Off"], ["llm", "Writer engine"], ["jeff", "Jeff guard adapter"]],
  },
];

const PLACEHOLDER: Record<string, { url: string; model: string }> = {
  ollama: { url: "http://127.0.0.1:11434", model: "qwen2.5:3b" },
  openai: { url: "https://openrouter.ai/api/v1", model: "google/gemini-2.5-flash" },
  jeff: { url: "http://127.0.0.1:8765", model: "ground" },
};

function RoleCard({ role, cur, onSaved }: { role: (typeof ROLES)[number]; cur: EngineRole; onSaved: () => void }) {
  const [kind, setKind] = useState(cur.kind);
  const [url, setUrl] = useState(cur.url);
  const [model, setModel] = useState(cur.model);
  const [agent, setAgent] = useState(cur.agent || "claude");
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => (setKind(cur.kind), setUrl(cur.url), setModel(cur.model)), [cur.kind, cur.url, cur.model]);
  const ph = PLACEHOLDER[kind] ?? { url: "", model: "" };
  const embedModel = role.id === "embed" && kind === "ollama" ? "nomic-embed-text" : ph.model;
  const guardModel = role.id === "guard" && kind === "jeff" ? "guard" : embedModel;
  const needsUrl = ["ollama", "openai", "jeff"].includes(kind);

  const save = async (test: boolean) => {
    setBusy(true);
    setMsg(null);
    try {
      await api.enginesConfigure(role.id, { kind, url: needsUrl ? url : "", model: needsUrl ? model : "", agent: kind === "agent" ? agent : "" });
      onSaved();
      if (test) setMsg({ ok: true, text: await api.enginesTest(role.id) });
      else setMsg({ ok: true, text: "Saved." });
    } catch (e) {
      setMsg({ ok: false, text: String(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="rounded-lg border border-white/10 p-3 text-xs">
      <div className="mb-1 flex items-center gap-2">
        <span className="font-display text-sm font-semibold">{role.title}</span>
        {cur.enabled ? <Badge tone="green">{cur.kind}</Badge> : <Badge>{cur.kind}</Badge>}
        {cur.cloud && <Badge tone="amber">sends data off this Mac</Badge>}
      </div>
      <div className="mb-2 text-[11px] leading-relaxed text-zinc-500">{role.what}</div>
      <div className="flex flex-wrap items-center gap-2">
        <select value={kind} onChange={(e) => setKind(e.target.value)} className="rounded-md border border-white/10 bg-black/40 px-2 py-1">
          {role.kinds.map(([k, label]) => (
            <option key={k} value={k}>
              {label}
            </option>
          ))}
        </select>
        {needsUrl && (
          <>
            <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder={ph.url} spellCheck={false} className="min-w-[14rem] flex-1 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1 font-mono text-[11px] outline-none focus:border-white/30" />
            <input value={model} onChange={(e) => setModel(e.target.value)} placeholder={guardModel} spellCheck={false} className="w-44 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1 font-mono text-[11px] outline-none focus:border-white/30" />
          </>
        )}
        {kind === "agent" && (
          <select value={agent} onChange={(e) => setAgent(e.target.value)} className="rounded-md border border-white/10 bg-black/40 px-2 py-1">
            {getCatalog()
              .filter((c) => c.wake === "cli")
              .map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
          </select>
        )}
        <button className={btn} disabled={busy} onClick={() => save(false)}>
          Save
        </button>
        <button className={btnPrimary} disabled={busy || kind === "off" || kind === "builtin"} onClick={() => save(true)}>
          {busy ? "Testing…" : "Save & test"}
        </button>
      </div>
      {msg && <div className={`mt-2 whitespace-pre-wrap text-[11px] ${msg.ok ? "text-emerald-300" : "text-red-300"}`}>{msg.text}</div>}
    </div>
  );
}

export default function EnginesPanel() {
  const [st, setSt] = useState<EnginesStatus | null>(null);
  const [stats, setStats] = useState<Record<string, unknown> | null>(null);
  const [key, setKey] = useState("");
  const [keyMsg, setKeyMsg] = useState<string | null>(null);
  const [cap, setCap] = useState("");
  const load = () => {
    api.enginesStatus().then(setSt).catch(() => {});
    api.routerStats().then(setStats).catch(() => {});
    api.getSetting("media.max_mb").then((v) => setCap(v ?? "50")).catch(() => {});
  };
  useEffect(load, []);
  if (!st) return <div className="text-xs text-zinc-500">Loading…</div>;
  const pct = (x: unknown) => (typeof x === "number" ? `${Math.round(x * 100)}%` : "—");

  return (
    <div className="space-y-3">
      <div className="rounded-lg border border-white/10 bg-white/[0.03] p-3 text-[11.5px] leading-relaxed text-zinc-400">
        All engines are optional and off by default; turned on, they're used fully. This Mac: <b className="text-zinc-200">{st.ramGb} GB</b>. {st.recommendation}
      </div>
      {ROLES.map((r) => (
        <RoleCard key={r.id} role={r} cur={st[r.id]} onSaved={load} />
      ))}

      <div className="rounded-lg border border-white/10 p-3 text-xs">
        <div className="mb-1 font-display text-sm font-semibold">Cloud key</div>
        <div className="mb-2 text-[11px] text-zinc-500">
          For "Cloud key" engines (OpenRouter, Gemini's OpenAI-compatible endpoint, any OpenAI-compatible API). Stored in the macOS Keychain, never in the vault.{" "}
          {st.apiKey ? <span className="text-emerald-300">A key is saved.</span> : <span>No key saved.</span>}
        </div>
        <div className="flex gap-2">
          <input type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder="sk-or-…" autoComplete="off" className="flex-1 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1 font-mono text-[11px] outline-none focus:border-white/30" />
          <button
            className={btnPrimary}
            disabled={!key.trim()}
            onClick={() =>
              api
                .enginesConfigure("writer", { apiKey: key.trim() })
                .then(() => (setKey(""), setKeyMsg("Saved to the Keychain."), load()))
                .catch((e) => setKeyMsg(String(e)))
            }
          >
            Save key
          </button>
          {st.apiKey && (
            <button className={btn} onClick={() => api.enginesConfigure("writer", { apiKey: "" }).then(() => (setKeyMsg("Key removed."), load()))}>
              Remove
            </button>
          )}
        </div>
        {keyMsg && <div className="mt-1 text-[11px] text-zinc-400">{keyMsg}</div>}
      </div>

      <div className="rounded-lg border border-white/10 p-3 text-xs">
        <div className="mb-2 font-display text-sm font-semibold">Routing: do we need a stronger decision model?</div>
        {stats ? (
          <div className="grid grid-cols-2 gap-x-6 gap-y-1 text-[11.5px] text-zinc-400">
            <div>Routing decisions</div>
            <div className="text-zinc-200">{String(stats.total ?? 0)}</div>
            <div>Asked you (unsure)</div>
            <div className="text-zinc-200">
              {String(stats.asked ?? 0)} ({pct(stats.askRate)})
            </div>
            <div>You answered</div>
            <div className="text-zinc-200">{String(stats.answered ?? 0)}</div>
            <div>Router's best guess was right</div>
            <div className="text-zinc-200">{pct(stats.agreement)}</div>
            <div>Decided by Jeff / the writer</div>
            <div className="text-zinc-200">{String(stats.byDecisionEngine ?? 0)}</div>
            {typeof stats.advice === "string" && stats.advice && <div className="col-span-2 mt-1 text-zinc-300">{stats.advice}</div>}
          </div>
        ) : (
          <div className="text-zinc-500">No decisions yet.</div>
        )}
      </div>

      <div className="flex items-center gap-2 rounded-lg border border-white/10 p-3 text-xs">
        <span className="font-display text-sm font-semibold">Media</span>
        <span className="text-zinc-500">Largest attachment</span>
        <input value={cap} onChange={(e) => setCap(e.target.value.replace(/\D/g, ""))} className="w-16 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1 text-right outline-none" />
        <span className="text-zinc-500">MB</span>
        <button className={btn} onClick={() => api.setSetting("media.max_mb", cap || "50").then(load)}>
          Save
        </button>
      </div>
    </div>
  );
}
