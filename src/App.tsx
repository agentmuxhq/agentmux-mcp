import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { FileText, FolderGit2, GitBranch, Plus, RefreshCw, Search, Send, Settings, Terminal } from "lucide-react";
import {
  agentMeta,
  ago,
  api,
  clock,
  inTauri,
  isGrok,
  onMuxEvent,
  setKnownProviders,
  type Message,
  type MuxEvent,
  type Overview,
  type Room,
} from "./lib/api";
import { Avatar, Badge, Dot, btn, btnPrimary } from "./components/ui";
import ConnectModal from "./components/ConnectModal";
import NewRoomModal from "./components/NewRoomModal";
import Starter from "./components/Starter";
import { UpdateRow } from "./components/UpdateControls";
import PublicAccessCard from "./components/PublicAccessCard";
import SettingsModal from "./components/SettingsModal";

type RightTab = "details" | "context" | "status";
type Reply = { status: string; text: string };

/** Room members first, then everyone else. */
function AgentOptions({ ov, members, exclude }: { ov: Overview; members: string[]; exclude?: string }) {
  const all = ov.providers.map((p) => p.id).filter((id) => id !== exclude);
  const inRoom = all.filter((id) => members.includes(id));
  const rest = all.filter((id) => !members.includes(id));
  const opt = (id: string) => (
    <option key={id} value={id}>
      {agentMeta(id).handle}
    </option>
  );
  return (
    <>
      {inRoom.map(opt)}
      {rest.length > 0 && (inRoom.length > 0 ? <optgroup label="Not in this room yet">{rest.map(opt)}</optgroup> : rest.map(opt))}
    </>
  );
}

function MessageRow({ m }: { m: Message }) {
  const system = m.sender === "agentmux";
  const name = m.sender === "user" ? "You" : system ? m.sender : agentMeta(m.sender).name;
  return (
    <div className="flex gap-3 px-6 py-3">
      <Avatar id={m.sender} />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2 text-xs">
          <span className="font-display font-semibold">{name}</span>
          {m.recipient && m.recipient !== "all" && <span className="text-zinc-500">→ @{m.recipient}</span>}
          <span className="text-zinc-600">{clock(m.createdAt)}</span>
          {m.type === "a2a_request" && <Badge>ask</Badge>}
          {m.type === "a2a_reply" && <Badge tone="green">reply</Badge>}
          {system && <Badge tone="red">error</Badge>}
        </div>
        <div className={`mt-1 select-text whitespace-pre-wrap break-words text-[13px] leading-relaxed ${system ? "text-red-300" : "text-zinc-200"}`}>{m.text}</div>
        {m.codeBlock && <pre className="mt-2 select-text overflow-x-auto rounded-md border border-white/10 bg-black/40 p-3 font-mono text-[11px] text-zinc-300">{m.codeBlock}</pre>}
      </div>
    </div>
  );
}

function ContextBlock({ label, icon, body }: { label: string; icon: React.ReactNode; body?: string }) {
  return (
    <div className="mb-4">
      <div className="mb-1.5 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">
        {icon}
        {label}
      </div>
      {body ? (
        <pre className="max-h-56 select-text overflow-auto whitespace-pre-wrap rounded-md border border-white/10 bg-black/40 p-2.5 font-mono text-[10.5px] leading-relaxed text-zinc-300">{body}</pre>
      ) : (
        <div className="rounded-md border border-dashed border-white/10 p-2.5 text-[11px] text-zinc-600">Nothing captured yet</div>
      )}
    </div>
  );
}

export default function App() {
  const [ov, setOv] = useState<Overview | null>(null);
  const [roomId, setRoomId] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [running, setRunning] = useState<Record<string, { from: string; to: string }>>({});
  const [replies, setReplies] = useState<Record<string, Reply>>({});
  const [log, setLog] = useState<MuxEvent[]>([]);
  const [rightTab, setRightTab] = useState<RightTab>("status");
  const [showConnect, setShowConnect] = useState(false);
  const [showSettings, setShowSettings] = useState<false | "daemon" | "access" | "sync">(false);
  const [error, setError] = useState<string | null>(null);

  const [to, setTo] = useState<string>("chatgpt");
  const [then, setThen] = useState<string>("none");
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);

  const [showNewRoom, setShowNewRoom] = useState(false);
  const [connectInit, setConnectInit] = useState<{ agent?: string; addBot?: boolean }>({});
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<{ roomId: string; sender: string; text: string; createdAt: string }[] | null>(null);
  const [notes, setNotes] = useState("");

  const roomRef = useRef<string | null>(null);
  const sessionRef = useRef<string | null>(null);
  const endRef = useRef<HTMLDivElement>(null);
  roomRef.current = roomId;
  sessionRef.current = sessionId;

  const room: Room | undefined = ov?.rooms.find((r) => r.id === roomId);

  const refresh = useCallback(async () => {
    try {
      const o = await api.overview();
      setKnownProviders(o.providers);
      setOv(o);
      setRoomId((cur) => (cur && o.rooms.some((r) => r.id === cur) ? cur : (o.rooms[0]?.id ?? null)));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const loadMessages = useCallback(async () => {
    if (!sessionRef.current) return setMessages([]);
    try {
      setMessages(await api.messages(sessionRef.current));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // Poll as a safety net (an external daemon doesn't emit events into this window).
  useEffect(() => {
    if (!inTauri()) return;
    refresh();
    // Events only come from this process. Agents connected to another AgentMux process (an
    // "external" daemon) or another synced Mac write straight to the vault, so poll the open session too.
    const t = setInterval(() => {
      refresh();
      loadMessages();
    }, 2500);
    return () => clearInterval(t);
  }, [refresh, loadMessages]);

  useEffect(() => {
    if (!inTauri()) return;
    let un: (() => void) | undefined;
    onMuxEvent((e) => {
      setLog((l) => [e, ...l].slice(0, 40));
      if (e.kind === "ticket") {
        const d = e.data;
        setRunning((r) => {
          const n = { ...r };
          if (d.status === "pending" || d.status === "running") n[d.id] = { from: d.from, to: d.to };
          else delete n[d.id];
          return n;
        });
      }
      if (e.kind === "message") {
        const d = e.data;
        if (d.ticketId && (d.type === "a2a_reply" || d.type === "system")) {
          setReplies((r) => ({ ...r, [d.ticketId]: { status: d.type === "system" ? "failed" : "answered", text: d.text } }));
        }
        if (d.sessionId === sessionRef.current) loadMessages();
      }
      refresh();
    }).then((u) => (un = u));
    return () => un?.();
  }, [refresh, loadMessages]);

  // The menu bar asks the window to open a screen.
  useEffect(() => {
    if (!inTauri()) return;
    const offs = [
      listen<string>("ui://settings", (e) => setShowSettings((e.payload as "daemon" | "access" | "sync") ?? "daemon")),
      listen("ui://connect", () => (setConnectInit({}), setShowConnect(true))),
      listen<string>("ui://error", (e) => setError(String(e.payload))),
    ];
    return () => void offs.forEach((o) => o.then((f) => f()));
  }, []);

  // Follow the newest session when the room changes or a new session appears.
  const latestSession = room?.sessions[0]?.id ?? null;
  useEffect(() => {
    setSessionId(latestSession);
  }, [roomId, latestSession]);
  useEffect(() => {
    loadMessages();
  }, [sessionId, loadMessages]);
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length]);
  useEffect(() => {
    if (!room || !ov) return;
    setTo((cur) => {
      const ids = ov.providers.map((p) => p.id);
      if (room.participants.includes(cur) || (room.participants.length === 0 && ids.includes(cur))) return cur;
      return room.participants[0] ?? ov.providers.find((p) => p.connectedAt)?.id ?? cur;
    });
    setThen("none");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId]);
  useEffect(() => {
    setNotes(room?.context.notes ?? "");
  }, [roomId, room?.context.notes]);

  const send = async () => {
    if (!room || !text.trim() || sending) return;
    setSending(true);
    setError(null);
    try {
      await api.send(room.id, sessionId, to, text.trim(), then);
      setText("");
    } catch (e) {
      setError(String(e));
    } finally {
      setSending(false);
      refresh();
    }
  };

  const doSearch = async () => {
    if (!query.trim()) return setHits(null);
    try {
      setHits(await api.search(query.trim()));
    } catch (e) {
      setError(String(e));
    }
  };

  if (!inTauri()) {
    return (
      <div className="flex h-screen items-center justify-center p-8 text-center text-sm text-zinc-400">
        AgentMux talks to its local daemon through Tauri. Run <code className="mx-1 rounded bg-white/10 px-1.5 py-0.5">bun run dev</code> to open the desktop app.
      </div>
    );
  }
  if (!ov) return <div className="flex h-screen items-center justify-center text-sm text-zinc-500">Starting AgentMux…</div>;

  const myRunning = Object.entries(running);
  const connectedCount = ov.providers.filter((p) => p.connectedAt).length;

  return (
    <div className="flex h-screen bg-[#0E0E0E] text-zinc-100">
      {/* ───────── Left: providers + rooms ───────── */}
      <aside className="flex w-72 shrink-0 flex-col border-r border-white/10 bg-[#121212]">
        <div data-tauri-drag-region className="h-12 shrink-0" />
        <div className="px-3 pb-3">
          <div className="flex items-center gap-2 rounded-md border border-white/10 bg-black/30 px-2.5 py-1.5">
            <Search size={13} className="text-zinc-500" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && doSearch()}
              placeholder="Search all threads"
              className="min-w-0 flex-1 select-text bg-transparent text-xs outline-none placeholder:text-zinc-600"
            />
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-2">
          <div className="mb-1 flex items-center justify-between px-2 pt-1">
            <span className="text-[11px] font-semibold uppercase tracking-wide text-zinc-500">Providers</span>
            <button onClick={() => (setConnectInit({}), setShowConnect(true))} className="rounded p-1 text-zinc-400 hover:bg-white/10 hover:text-white" title="Connect a provider">
              <Plus size={14} />
            </button>
          </div>
          {ov.providers.map((p) => (
            <button key={p.id} onClick={() => (setConnectInit({ agent: p.id }), setShowConnect(true))} className="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left hover:bg-white/5">
              <Avatar id={p.id} size={26} />
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px] font-medium">{agentMeta(p.id).name}</div>
                <div className="truncate text-[11px] text-zinc-500">
                  {p.description || (p.connectedAt ? `seen ${ago(p.lastSeen)}` : isGrok(p.id) && !p.registered ? "not connected · Grok Bot" : "not connected")}
                </div>
              </div>
              <Dot on={!!p.connectedAt} />
            </button>
          ))}
          <button
            onClick={() => (setConnectInit({ agent: "grok", addBot: true }), setShowConnect(true))}
            className="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-[12px] text-zinc-500 hover:bg-white/5 hover:text-zinc-300"
          >
            <span className="flex h-[26px] w-[26px] items-center justify-center rounded-full border border-dashed border-white/20">
              <Plus size={12} />
            </span>
            Add Grok bot
          </button>

          <div className="mb-1 mt-4 flex items-center justify-between px-2">
            <span className="text-[11px] font-semibold uppercase tracking-wide text-zinc-500">Bridge rooms</span>
            <button onClick={() => setShowNewRoom(true)} className="rounded p-1 text-zinc-400 hover:bg-white/10 hover:text-white" title="Add a workspace">
              <Plus size={14} />
            </button>
          </div>
          {ov.rooms.length === 0 && (
            <div className="px-2 py-2 text-[11px] leading-relaxed text-zinc-600">
              Rooms appear when an agent works in a repo, or add one with +. You choose the folder and which providers take part.
            </div>
          )}
          {ov.rooms.map((r) => (
            <button
              key={r.id}
              onClick={() => {
                setRoomId(r.id);
                setHits(null);
              }}
              className={`mb-0.5 flex w-full items-center gap-2.5 rounded-md px-2 py-2 text-left ${r.id === roomId ? "bg-white/10" : "hover:bg-white/5"}`}
            >
              <FolderGit2 size={15} className="shrink-0 text-zinc-500" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px] font-medium">{r.title}</div>
                <div className="flex items-center gap-1 text-[11px] text-zinc-500">
                  <span className="truncate">{r.branch || "no git"}</span>
                  <span>·</span>
                  <span>{ago(r.updatedAt)}</span>
                </div>
              </div>
              <div className="flex -space-x-1.5">
                {r.participants.slice(0, 3).map((p) => (
                  <div key={p} className="rounded-full ring-2 ring-[#121212]">
                    <Avatar id={p} size={16} />
                  </div>
                ))}
              </div>
            </button>
          ))}
        </div>

        <UpdateRow />
        <button onClick={() => setShowSettings("daemon")} className="flex items-center gap-2.5 border-t border-white/10 px-4 py-3 text-xs text-zinc-400 hover:bg-white/5 hover:text-white">
          <Settings size={14} /> Settings
          <span className="ml-auto text-[11px] text-zinc-600">{connectedCount}/4 connected</span>
        </button>
      </aside>

      {/* ───────── Center: sessions + stream ───────── */}
      <main className="flex min-w-0 flex-1 flex-col">
        <div data-tauri-drag-region className="flex h-12 shrink-0 items-center gap-3 border-b border-white/10 px-6">
          {room ? (
            <>
              <span className="pointer-events-none font-display text-sm font-semibold">{room.title}</span>
              <span className="pointer-events-none flex items-center gap-1 text-xs text-zinc-500">
                <GitBranch size={12} />
                {room.branch || "no git"}
              </span>
              <button
                className={`${btn} ml-auto`}
                onClick={async () => {
                  const id = await api.newSession(room.id, "").catch((e) => (setError(String(e)), null));
                  if (id) setSessionId(id);
                  refresh();
                }}
              >
                <Plus size={12} /> New session
              </button>
            </>
          ) : (
            <span className="pointer-events-none text-sm text-zinc-500">AgentMux</span>
          )}
        </div>

        {room && room.sessions.length > 0 && (
          <div className="flex gap-1 overflow-x-auto border-b border-white/10 px-4 py-2">
            {room.sessions.map((s) => (
              <button
                key={s.id}
                onClick={() => setSessionId(s.id)}
                className={`shrink-0 rounded-md px-3 py-1 text-xs ${s.id === sessionId ? "bg-white/10 text-white" : "text-zinc-500 hover:bg-white/5"}`}
              >
                {s.title}
              </button>
            ))}
          </div>
        )}

        <div className="min-h-0 flex-1 overflow-y-auto">
          {hits ? (
            <div className="p-6">
              <div className="mb-3 text-xs text-zinc-500">{hits.length} results for “{query}”</div>
              {hits.map((h, i) => (
                <button key={i} onClick={() => (setRoomId(h.roomId), setHits(null))} className="mb-2 block w-full rounded-md border border-white/10 p-3 text-left hover:bg-white/5">
                  <div className="text-[11px] text-zinc-500">
                    {h.roomId} · {h.sender} · {ago(h.createdAt)}
                  </div>
                  <div className="mt-1 line-clamp-3 text-xs">{h.text}</div>
                </button>
              ))}
            </div>
          ) : !room ? (
            <div className="mx-auto mt-24 max-w-md px-6 text-center">
              <h1 className="font-display text-xl font-semibold">Message your agents, not models.</h1>
              <p className="mt-3 text-sm leading-relaxed text-zinc-400">
                Connect Cursor, Claude Code, ChatGPT and Grok, then let them ask each other. Everything stays in <span className="font-mono text-zinc-300">~/.agentmux</span>.
              </p>
              <div className="mt-6 flex justify-center gap-2">
                <button className={btnPrimary} onClick={() => setShowConnect(true)}>
                  Connect providers
                </button>
                <button className={btn} onClick={() => setShowNewRoom(true)}>
                  Add a workspace
                </button>
              </div>
            </div>
          ) : messages.length === 0 ? (
            <Starter ov={ov} room={room} onPick={(id) => setTo(id)} />
          ) : (
            <>
              {messages.map((m) => (
                <MessageRow key={m.id} m={m} />
              ))}
              <div ref={endRef} />
            </>
          )}
        </div>

        {myRunning.length > 0 && (
          <div className="border-t border-white/10 px-6 py-2 text-xs text-amber-300">
            {myRunning.map(([id, t]) => (
              <div key={id} className="flex items-center gap-2">
                <Dot on={false} pulse /> @{t.to} is working on @{t.from}&apos;s question… <span className="font-mono text-zinc-600">{id}</span>
              </div>
            ))}
          </div>
        )}
        {error && (
          <div className="flex items-start gap-2 border-t border-red-500/20 bg-red-500/10 px-6 py-2 text-xs text-red-200">
            <span className="min-w-0 flex-1 break-words">{error}</span>
            <button onClick={() => setError(null)} className="shrink-0 text-red-300 hover:text-white">
              dismiss
            </button>
          </div>
        )}

        {room && (
          <div className="border-t border-white/10 p-4">
            <div className="rounded-xl border border-white/10 bg-[#161616] p-3">
              <textarea
                value={text}
                onChange={(e) => setText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) send();
                }}
                placeholder={`Ask @${to} something about ${room.title}…  (⌘↵ to send)`}
                rows={2}
                className="w-full resize-none select-text bg-transparent text-[13px] outline-none placeholder:text-zinc-600"
              />
              <div className="mt-2 flex items-center gap-2 text-xs">
                <span className="text-zinc-500">To</span>
                <select value={to} onChange={(e) => setTo(e.target.value)} className="rounded-md border border-white/10 bg-black/40 px-2 py-1">
                  <AgentOptions ov={ov} members={room.participants} />
                </select>
                <span className="text-zinc-500">then →</span>
                <select value={then} onChange={(e) => setThen(e.target.value)} className="rounded-md border border-white/10 bg-black/40 px-2 py-1">
                  <option value="none">nobody</option>
                  <AgentOptions ov={ov} members={room.participants} exclude={to} />
                </select>
                <button className={`${btnPrimary} ml-auto`} disabled={!text.trim() || sending} onClick={send}>
                  <Send size={12} /> Send
                </button>
              </div>
            </div>
          </div>
        )}
      </main>

      {/* ───────── Right: inspector ───────── */}
      <aside className="flex w-80 shrink-0 flex-col border-l border-white/10 bg-[#121212]">
        <div data-tauri-drag-region className="flex h-12 shrink-0 items-end gap-1 border-b border-white/10 px-3 pb-2">
          {(["details", "context", "status"] as RightTab[]).map((t) => (
            <button key={t} onClick={() => setRightTab(t)} className={`rounded-md px-3 py-1 text-xs capitalize ${rightTab === t ? "bg-white/10" : "text-zinc-500 hover:bg-white/5"}`}>
              {t === "context" ? "Shared context" : t}
            </button>
          ))}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {rightTab === "details" &&
            (room ? (
              <div className="space-y-5 text-xs">
                <div>
                  <div className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">Workspace</div>
                  <div className="break-all font-mono text-[11px] text-zinc-300">{room.workspacePath}</div>
                </div>
                <div>
                  <div className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">Participants</div>
                  {room.participants.length === 0 && <div className="text-zinc-600">None yet</div>}
                  {room.participants.map((p) => (
                    <div key={p} className="mb-1.5 flex items-center gap-2">
                      <Avatar id={p} size={20} /> {agentMeta(p).name}
                      <Badge>{({ cli: "headless", webhook: "webhook" } as Record<string, string>)[ov.providers.find((x) => x.id === p)?.wakeKind ?? ""] ?? "inbox"}</Badge>
                    </div>
                  ))}
                </div>
                <div>
                  <div className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">Permissions</div>
                  <label className="flex cursor-pointer items-start gap-2">
                    <input
                      type="checkbox"
                      checked={room.allowEdits}
                      onChange={(e) => api.setAllowEdits(room.id, e.target.checked).then(refresh)}
                      className="mt-0.5"
                    />
                    <span className="text-zinc-300">
                      Let woken agents edit files
                      <span className="mt-0.5 block text-[11px] text-zinc-600">Off = read-only (Cursor ask mode, Claude plan mode, Codex read-only sandbox).</span>
                    </span>
                  </label>
                </div>
                <div>
                  <div className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">Sessions</div>
                  {room.sessions.map((s) => (
                    <button key={s.id} onClick={() => setSessionId(s.id)} className="mb-1 block w-full truncate rounded px-2 py-1 text-left hover:bg-white/5">
                      {s.title}
                    </button>
                  ))}
                </div>
              </div>
            ) : (
              <div className="text-xs text-zinc-600">Select a room.</div>
            ))}

          {rightTab === "context" &&
            (room ? (
              <div>
                <div className="mb-3 flex gap-2">
                  <button className={btn} onClick={() => api.refreshGit(room.id).then(refresh).catch((e) => setError(String(e)))}>
                    <RefreshCw size={12} /> Refresh git diff
                  </button>
                </div>
                <ContextBlock label="git diff HEAD" icon={<GitBranch size={12} />} body={room.context.diff} />
                <ContextBlock label="Terminal" icon={<Terminal size={12} />} body={room.context.terminal} />
                <div className="mb-1.5 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">
                  <FileText size={12} /> Shared notes
                </div>
                <textarea
                  value={notes}
                  onChange={(e) => setNotes(e.target.value)}
                  onBlur={() => notes !== (room.context.notes ?? "") && api.setContext(room.id, "notes", notes).then(refresh)}
                  rows={6}
                  placeholder="Notes every agent in this room can read (mux_read_context)"
                  className="w-full select-text resize-none rounded-md border border-white/10 bg-black/40 p-2.5 font-mono text-[11px] outline-none focus:border-white/30"
                />
                <div className="mt-3 text-[11px] leading-relaxed text-zinc-600">
                  Terminal output comes from <span className="font-mono">agentmux run -- &lt;cmd&gt;</span> or an agent calling mux_publish_context.
                </div>
              </div>
            ) : (
              <div className="text-xs text-zinc-600">Select a room.</div>
            ))}

          {rightTab === "status" && (
            <div className="space-y-5 text-xs">
              <div className="rounded-lg border border-white/10 p-3">
                <div className="mb-2 flex items-center gap-2">
                  <Dot on={ov.daemon === "embedded" || ov.daemon === "external"} />
                  <span className="font-display text-sm font-semibold">Local daemon</span>
                  <Badge tone={ov.daemon.startsWith("error") ? "red" : ov.daemon === "off" ? "zinc" : "green"}>{ov.daemon}</Badge>
                  <button
                    className={`${btn} ml-auto`}
                    onClick={() => api.setDaemon(ov.daemon === "off" || ov.daemon.startsWith("error")).then(refresh).catch((e) => setError(String(e)))}
                  >
                    {ov.daemon === "off" || ov.daemon.startsWith("error") ? "Turn on" : "Turn off"}
                  </button>
                </div>
                <div className="break-all font-mono text-[10.5px] text-zinc-500">{ov.daemon === "off" ? "Off: agents can't reach AgentMux, and the tunnel is down." : ov.socket}</div>
              </div>
              <PublicAccessCard onManage={() => setShowSettings("access")} />
              <div>
                <div className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">Handshakes</div>
                {ov.providers.map((p) => (
                  <div key={p.id} className="mb-1.5 flex items-center gap-2">
                    <Dot on={!!p.connectedAt} />
                    <span className="flex-1">{agentMeta(p.id).name}</span>
                    <span className="text-zinc-600">{p.connectedAt ? `${p.calls} calls · ${ago(p.lastSeen)}` : "—"}</span>
                  </div>
                ))}
              </div>
              <div>
                <div className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-zinc-500">Live events</div>
                {log.length === 0 && <div className="text-zinc-600">Waiting for agent traffic…</div>}
                <div className="space-y-1 font-mono text-[10.5px] text-zinc-500">
                  {log.map((e, i) => (
                    <div key={i} className="truncate">
                      <span className="text-zinc-300">{e.kind}</span>{" "}
                      {e.kind === "ticket"
                        ? `${e.data.from}→${e.data.to} ${e.data.status}`
                        : e.kind === "message"
                          ? `${e.data.sender}: ${String(e.data.text).slice(0, 40)}`
                          : e.kind === "tunnel"
                            ? `${e.data.state}${e.data.url ? ` · ${String(e.data.url).replace("https://", "")}` : ""}`
                            : e.kind === "provider_connected"
                              ? agentMeta(e.data.id).name
                              : e.kind === "room"
                                ? String(e.data.id)
                                : JSON.stringify(e.data)}
                    </div>
                  ))}
                </div>
              </div>
            </div>
          )}
        </div>
      </aside>

      {showConnect && <ConnectModal ov={ov} initial={connectInit.agent} startAdding={connectInit.addBot} replies={replies} onClose={() => setShowConnect(false)} onChanged={refresh} />}
      {showNewRoom && (
        <NewRoomModal
          ov={ov}
          onClose={() => setShowNewRoom(false)}
          onCreated={(id) => {
            setShowNewRoom(false);
            setRoomId(id);
            setHits(null);
            refresh();
          }}
        />
      )}
      {showSettings && <SettingsModal ov={ov} initial={showSettings} onClose={() => setShowSettings(false)} />}
    </div>
  );
}
