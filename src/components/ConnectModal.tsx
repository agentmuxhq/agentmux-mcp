import { useEffect, useState } from "react";
import { Check, Copy, Radio } from "lucide-react";
import { agentMeta, ago, api, getCatalog, isGrok, providerOf, providerState, type Detect, type GrokWebhook, type Overview } from "../lib/api";
import TunnelPanel from "./TunnelPanel";
import { Avatar, Badge, Dot, Modal, btn, btnPrimary } from "./ui";

interface Props {
  ov: Overview;
  initial?: string;
  startAdding?: boolean;
  replies: Record<string, { status: string; text: string }>;
  onClose: () => void;
  onChanged: () => void;
}

export default function ConnectModal({ ov, initial = "cursor", startAdding, replies, onClose, onChanged }: Props) {
  // The tab is a provider type; a multi-instance one (Grok) has one entry per bot, picked separately.
  const [sel, setSel] = useState<string>(providerOf(initial)?.id ?? initial);
  const [bot, setBot] = useState(isGrok(initial) ? initial : "grok");
  const [adding, setAdding] = useState(!!startAdding);
  const [newName, setNewName] = useState("");
  const [renameTo, setRenameTo] = useState("");
  const [detect, setDetect] = useState<Record<string, Detect>>({});
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [ticket, setTicket] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [gk, setGk] = useState<GrokWebhook | null>(null);
  const [whUrl, setWhUrl] = useState("");
  const [whKey, setWhKey] = useState("");

  const load = () => api.detect().then((d) => setDetect(Object.fromEntries(d.map((x) => [x.agent, x])))).catch(() => {});
  useEffect(() => {
    load();
  }, []);
  useEffect(() => {
    setMsg(null);
    setTicket(null);
  }, [sel]);

  useEffect(() => {
    setGk(null);
    if (sel === "grok") api.grokStatus(bot).then(setGk).catch(() => {});
  }, [sel, bot]);

  const bots = ov.providers.filter((x) => isGrok(x.id));
  const target = sel === "grok" ? bot : sel; // the agent this tab is configuring
  useEffect(() => {
    setRenameTo(agentMeta(target).name);
  }, [target, ov.providers]);

  const p = ov.providers.find((x) => x.id === target);
  const d = detect[sel];
  const cat = providerOf(sel);
  const catalog = ov.catalog?.length ? ov.catalog : getCatalog();
  // Remote providers (Grok) are added in their own app's settings, not by AgentMux.
  const manual = !!cat?.remoteAccess;
  const handshake = !!p?.connectedAt;
  // The ping's answer: from this window's events, or by polling the vault when another AgentMux
  // process (the background service) is the one handling it.
  const [polled, setPolled] = useState<{ status: string; text: string } | undefined>(undefined);
  useEffect(() => {
    setPolled(undefined);
    if (!ticket) return;
    const t = setInterval(() => {
      api
        .ticketStatus(ticket)
        .then((v) => {
          if (v && (v.status === "answered" || v.status === "failed")) {
            setPolled({ status: v.status, text: v.answer ?? "" });
            clearInterval(t);
          }
        })
        .catch(() => {});
    }, 2000);
    return () => clearInterval(t);
  }, [ticket]);
  const reply = ticket ? (replies[ticket] ?? polled) : undefined;
  const meta = agentMeta(target);

  const run = async (fn: () => Promise<string>, label: string) => {
    setBusy(true);
    setMsg(null);
    try {
      const text = await fn();
      setMsg({ ok: true, text });
    } catch (e) {
      setMsg({ ok: false, text: `${label} failed: ${e}` });
    } finally {
      setBusy(false);
      load();
      onChanged();
    }
  };

  const copy = (what: string, value: string) => {
    navigator.clipboard?.writeText(value).then(() => {
      setCopied(what);
      setTimeout(() => setCopied(null), 1500);
    });
  };

  const saveWebhook = async () => {
    setBusy(true);
    setMsg(null);
    try {
      setGk(await api.grokSave(bot, whUrl.trim(), whKey));
      setWhUrl("");
      setWhKey(""); // the key now lives only in the macOS Keychain
      setMsg({ ok: true, text: "Webhook saved. The key is stored in the macOS Keychain, not in AgentMux's vault." });
    } catch (e) {
      setMsg({ ok: false, text: `Save failed: ${e}` });
    } finally {
      setBusy(false);
      onChanged();
    }
  };

  const tileState = (id: string, seen: boolean) => (detect[id] || providerOf(id)?.remoteAccess ? providerState(id, seen, detect[id]) : { label: "checking…", on: false });
  const registered = manual || !!d?.configured;

  return (
    <Modal title="Connect a provider" onClose={onClose}>
      <div className="mb-5 grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-2">
        {catalog.map(({ id, name, multiInstance }) => {
          const group = ov.providers.filter((x) => (multiInstance ? providerOf(x.id)?.id === id : x.id === id));
          const st = tileState(id, group.some((x) => x.connectedAt));
          return (
            <button
              key={id}
              onClick={() => setSel(id)}
              className={`flex flex-col items-center gap-2 rounded-lg border px-2 py-3 text-xs ${
                sel === id ? "border-white/30 bg-white/10" : "border-white/10 hover:bg-white/5"
              }`}
            >
              <Avatar id={id} size={32} />
              <span className="font-medium">{name}</span>
              <span className="flex items-center gap-1 text-[10px] text-zinc-500">
                <Dot on={st.on} pulse={st.pulse} />
                {st.label}
              </span>
            </button>
          );
        })}
      </div>

      <div className="space-y-4 text-sm">
        <div className="rounded-lg border border-white/10 p-3 text-xs">
          <div className="mb-2 flex flex-wrap items-center gap-2">
            <span className="font-display text-sm font-semibold">{meta.name}</span>
            <Badge tone={d?.installed ? "green" : "red"}>{d?.installed ? "detected" : "not found"}</Badge>
            <Badge tone={d?.configured ? "green" : "zinc"}>{d?.configured ? "registered" : "not registered"}</Badge>
            <Badge>{p?.wakeKind === "cli" ? "wake: headless CLI" : p?.wakeKind === "webhook" ? "wake: webhook" : "wake: inbox only"}</Badge>
          </div>
          <div className="text-zinc-500">{cat?.hint}</div>
          {d && <div className="mt-1 break-all font-mono text-[11px] text-zinc-500">{d.detail}</div>}
        </div>

        <div className="flex flex-wrap gap-2">
          <button className={btnPrimary} disabled={busy || manual} title={manual ? `${cat?.name} is configured by hand — see the steps below` : undefined} onClick={() => run(() => api.connect(target), "Connect")}>
            {d?.configured ? "Re-register" : "Connect"}
          </button>
          {!manual && d?.configured && (
            <button className={btn} disabled={busy} onClick={() => run(() => api.disconnect(target), "Disconnect")}>
              Disconnect
            </button>
          )}
          <button
            className={btn}
            disabled={busy || !handshake || !registered}
            title={handshake ? "Wake this agent and wait for its reply" : "Needs a completed handshake first"}
            onClick={async () => {
              setMsg(null);
              try {
                setTicket(await api.ping(target));
              } catch (e) {
                setMsg({ ok: false, text: `${e}` });
              }
            }}
          >
            <Radio size={13} /> Send test ping
          </button>
        </div>

        {msg && (
          <pre className={`whitespace-pre-wrap rounded-md p-3 font-mono text-[11px] ${msg.ok ? "bg-emerald-500/10 text-emerald-200" : "bg-red-500/10 text-red-200"}`}>
            {msg.text}
          </pre>
        )}

        {d?.contextHook && (
          <label className="flex cursor-pointer items-start gap-2 rounded-lg border border-white/10 p-3 text-xs">
            <input
              type="checkbox"
              className="mt-0.5"
              checked={!!d.options?.hook}
              disabled={busy}
              onChange={(e) => run(() => api.setProviderOption(sel, "hook", e.target.checked), "Hook")}
            />
            <span className="text-zinc-300">
              Tell {cat?.name} what&apos;s new each time you send a message
              <span className="mt-0.5 block text-[11px] leading-relaxed text-zinc-500">
                Adds a hook to {sel === "gemini" ? "~/.gemini/settings.json" : "~/.claude/settings.json"}. When another agent answers after {cat?.name} stopped waiting, the reply shows up in the same chat on your next message.
                AgentMux&apos;s own headless runs skip it.
              </span>
            </span>
          </label>
        )}

        {sel === "grok" && (
          <div className="space-y-3 rounded-lg border border-white/10 p-3 text-xs text-zinc-400">
            <div className="font-semibold text-zinc-200">Grok bots</div>
            <div className="text-zinc-500">
              Each Grok bot is its own contact with its own token, webhook and inbox, so answers never mix. Add one per bot you want to reach.
            </div>
            <div className="flex flex-wrap items-center gap-1.5">
              {bots.map((b) => (
                <button
                  key={b.id}
                  onClick={() => (setBot(b.id), setAdding(false), setMsg(null), setTicket(null))}
                  className={`flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] ${bot === b.id && !adding ? "border-white/30 bg-white/10 text-zinc-100" : "border-white/10 hover:bg-white/5"}`}
                >
                  <Dot on={!!b.connectedAt} />
                  {agentMeta(b.id).name}
                </button>
              ))}
              <button onClick={() => setAdding(true)} className={`rounded-full border border-dashed px-2.5 py-1 text-[11px] ${adding ? "border-white/40 text-zinc-100" : "border-white/20 text-zinc-400 hover:bg-white/5"}`}>
                + Add bot
              </button>
            </div>
            {adding ? (
              <div className="flex gap-2">
                <input
                  autoFocus
                  value={newName}
                  onChange={(e) => setNewName(e.target.value)}
                  placeholder="Bot name, e.g. Job Hunter"
                  className="min-w-0 flex-1 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1.5 text-[11px] text-zinc-200 outline-none focus:border-white/30"
                />
                <button
                  className={btnPrimary}
                  disabled={busy || !newName.trim()}
                  onClick={async () => {
                    setBusy(true);
                    try {
                      const id = await api.addBot(newName.trim());
                      setBot(id);
                      setNewName("");
                      setAdding(false);
                      setMsg({ ok: true, text: `Added ${newName.trim()} (${id}). Now give it a connector token and a routine below.` });
                    } catch (e) {
                      setMsg({ ok: false, text: `${e}` });
                    } finally {
                      setBusy(false);
                      onChanged();
                    }
                  }}
                >
                  Add
                </button>
                <button className={btn} onClick={() => setAdding(false)}>
                  Cancel
                </button>
              </div>
            ) : (
              <>
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-mono text-[11px] text-zinc-500">{bot}</span>
                  <input
                    value={renameTo}
                    onChange={(e) => setRenameTo(e.target.value)}
                    className="min-w-0 flex-1 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1 text-[11px] text-zinc-200 outline-none focus:border-white/30"
                  />
                  <button
                    className={btn}
                    disabled={busy || !renameTo.trim() || renameTo.trim() === meta.name}
                    onClick={() => api.renameAgent(bot, renameTo.trim()).then(onChanged).catch((e) => setMsg({ ok: false, text: `${e}` }))}
                  >
                    Rename
                  </button>
                  {bot !== "grok" && (
                    <button
                      className={btn}
                      disabled={busy}
                      onClick={async () => {
                        if (!window.confirm(`Remove ${meta.name}? Its token and webhook key are deleted.`)) return;
                        try {
                          await api.removeBot(bot);
                          setBot("grok");
                        } catch (e) {
                          setMsg({ ok: false, text: `${e}` });
                        }
                        onChanged();
                      }}
                    >
                      Remove bot
                    </button>
                  )}
                </div>
                <label className="flex cursor-pointer items-start gap-2 rounded-md border border-white/10 p-2.5">
                  <input
                    type="checkbox"
                    className="mt-0.5"
                    checked={!!p?.canAsk}
                    onChange={(e) => api.setBotCanAsk(bot, e.target.checked).then(onChanged).catch((err) => setMsg({ ok: false, text: `${err}` }))}
                  />
                  <span className="text-zinc-300">
                    Let this bot start conversations with other agents
                    <span className="mt-0.5 block text-[11px] leading-relaxed text-zinc-500">
                      On by default: it can ask Cursor, Claude Code, ChatGPT or other bots, up to 30 questions an hour. Off: it can only answer questions. Woken agents stay read-only, and its questions never carry your repo&apos;s git diff or notes.
                    </span>
                  </span>
                </label>
                {meta.subtitle === null && !p?.registered && <div className="text-zinc-600">The bot names itself on its first run, once its routine calls mux_register.</div>}
              </>
            )}

            <div className="font-semibold text-zinc-200">Set up {meta.name} in 3 steps</div>

            <div>
              <div className="mb-1 text-zinc-300">
                1 · Grok runs in the cloud, so it reaches AgentMux over HTTPS. Start a tunnel, then add its URL and token as a custom MCP connector in Grok:
              </div>
              <TunnelPanel compact agent={bot} />
            </div>

            <div>
              <div className="mb-1 text-zinc-300">2 · Create a Routine in Grok Bot with a Webhook trigger, and paste these instructions:</div>
              <div className="flex items-start gap-2 rounded bg-black/40 p-2 text-[11px] leading-relaxed text-zinc-300">
                <span className="min-w-0 flex-1 select-text">{gk?.instructions}</span>
                <button onClick={() => gk && copy("instr", gk.instructions)} className="shrink-0 text-zinc-400 hover:text-white">
                  {copied === "instr" ? <Check size={14} /> : <Copy size={14} />}
                </button>
              </div>
            </div>

            <div>
              <div className="mb-1 text-zinc-300">3 · Paste the routine&apos;s webhook URL and key:</div>
              {gk?.configured && (
                <div className="mb-2 flex items-center gap-2 text-emerald-300">
                  <Check size={13} /> Wake via webhook · <span className="font-mono">{gk.url}</span> · key in Keychain
                </div>
              )}
              <input
                value={whUrl}
                onChange={(e) => setWhUrl(e.target.value)}
                placeholder="https://…/automations/webhook/<routine-id>"
                spellCheck={false}
                className="mb-2 w-full select-text rounded-md border border-white/10 bg-black/40 px-2 py-1.5 font-mono text-[11px] text-zinc-200 outline-none focus:border-white/30"
              />
              <input
                type="password"
                value={whKey}
                onChange={(e) => setWhKey(e.target.value)}
                placeholder="Webhook key"
                autoComplete="off"
                className="mb-2 w-full select-text rounded-md border border-white/10 bg-black/40 px-2 py-1.5 font-mono text-[11px] text-zinc-200 outline-none focus:border-white/30"
              />
              <div className="flex flex-wrap gap-2">
                <button className={btnPrimary} disabled={busy || !whUrl.trim() || !whKey} onClick={saveWebhook}>
                  Save webhook
                </button>
                <button
                  className={btn}
                  disabled={busy || !gk?.configured}
                  title="Fires the routine once with a no-op message (uses one Grok run)"
                  onClick={() => run(() => api.grokTest(bot), "Webhook test")}
                >
                  Test webhook
                </button>
                {gk?.configured && (
                  <button
                    className={btn}
                    disabled={busy}
                    onClick={async () => {
                      try {
                        setGk(await api.grokClear(bot));
                        setMsg({ ok: true, text: "Webhook removed; Grok is inbox-only again." });
                      } catch (e) {
                        setMsg({ ok: false, text: `${e}` });
                      }
                      onChanged();
                    }}
                  >
                    Remove
                  </button>
                )}
              </div>
            </div>

            <div className="text-zinc-500">
              Without a webhook, questions to <span className="font-mono">@{bot}</span> wait in its inbox until Grok calls <span className="font-mono">mux_inbox</span>. A webhook only starts a
              run; Grok answers by calling <span className="font-mono">mux_reply</span>, so step 1 is required. Quick tunnels get a new URL each time they start, so update the connector in Grok when it changes (ngrok with a reserved domain keeps one URL).
            </div>
          </div>
        )}

        <div className="flex items-center gap-2 rounded-lg border border-white/10 p-3 text-xs">
          <Dot on={registered && handshake} pulse={registered && !handshake} />
          {d && !registered ? (
            <span className="text-zinc-400">
              {d.installed ? <>AgentMux isn&apos;t registered in {meta.name}. Click <b>Connect</b> to add it.</> : <>{meta.name} wasn&apos;t found on this Mac.</>}
            </span>
          ) : handshake ? (
            <span className="text-emerald-300">
              Handshake received · last seen {ago(p?.lastSeen)} · {p?.calls} calls
            </span>
          ) : (
            <span className="text-zinc-400">
              Waiting for first MCP handshake from {meta.name} on <span className="font-mono">{ov.socket.replace(ov.home, "~/.agentmux")}</span>… restart the app after
              registering.
            </span>
          )}
        </div>

        {ticket && (
          <div className="rounded-lg border border-white/10 p-3 text-xs">
            <div className="mb-1 font-semibold">Test ping · {ticket}</div>
            {!reply ? (
              <div className="text-zinc-400">Waking {meta.name}… this can take up to a minute.</div>
            ) : (
              <pre className={`whitespace-pre-wrap font-mono text-[11px] ${reply.status === "failed" ? "text-red-300" : "text-emerald-200"}`}>{reply.text}</pre>
            )}
          </div>
        )}
      </div>
    </Modal>
  );
}
