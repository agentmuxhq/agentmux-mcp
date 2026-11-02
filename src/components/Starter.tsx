import { useEffect, useMemo, useState } from "react";
import { Check, Copy } from "lucide-react";
import { agentMeta, isGrok, type Overview, type Room } from "../lib/api";
import { Avatar } from "./ui";

function CopyBlock({ label, text }: { label: string; text: string }) {
  const [done, setDone] = useState(false);
  return (
    <div>
      <div className="mb-1 text-[11px] text-zinc-500">{label}</div>
      <div className="flex items-start gap-2 rounded-lg border border-white/10 bg-black/40 p-2.5">
        <span className="min-w-0 flex-1 select-text break-words text-[12px] leading-relaxed text-zinc-200">{text}</span>
        <button
          title="Copy"
          className="shrink-0 text-zinc-400 hover:text-white"
          onClick={() =>
            navigator.clipboard?.writeText(text).then(() => {
              setDone(true);
              setTimeout(() => setDone(false), 1500);
            })
          }
        >
          {done ? <Check size={14} /> : <Copy size={14} />}
        </button>
      </div>
    </div>
  );
}

const sel = "rounded-md border border-white/10 bg-black/40 px-2 py-1 text-xs";

/** Empty-session guidance: how to start a conversation, from here or from another app's chat. */
export default function Starter({ ov, room, onPick }: { ov: Overview; room: Room; onPick: (agent: string) => void }) {
  // Everyone who can plausibly take part: room members plus connected providers.
  const pool = useMemo(() => {
    const ids = new Set<string>(room.participants);
    ov.providers.filter((p) => p.connectedAt).forEach((p) => ids.add(p.id));
    return [...ids].filter((id) => ov.providers.some((p) => p.id === id));
  }, [room.participants, ov.providers]);

  // Grok bots can answer but can't start asks over the tunnel, so they aren't asker choices.
  const askers = pool.filter((id) => !isGrok(id));
  const [from, setFrom] = useState(askers[0] ?? "chatgpt");
  const targets = pool.filter((id) => id !== from);
  const [target, setTarget] = useState(targets[0] ?? "claude");
  const [question, setQuestion] = useState("");
  const [third, setThird] = useState("");

  useEffect(() => {
    if (!askers.includes(from) && askers[0]) setFrom(askers[0]);
  }, [askers, from]);
  useEffect(() => {
    if (!targets.includes(target) && targets[0]) setTarget(targets[0]);
  }, [targets, target]);

  const q = question.trim() || `what's the status of ${room.title}?`;
  const to = agentMeta(target);
  const other = pool.find((id) => id !== from && id !== target);
  const base = `Use the agentmux tool mux_ask_peer with room "${room.id}" to ask ${target}: "${q}"`;
  const thenWho = third || other;

  return (
    <div className="mx-auto mt-10 max-w-xl px-6">
      <h2 className="font-display text-base font-semibold">Start the conversation</h2>
      <p className="mt-1 text-xs leading-relaxed text-zinc-500">
        Agents don't connect to each other. Each one talks to AgentMux, and AgentMux wakes the one being asked and brings the answer back. Pick who asks whom, then paste the prompt into that app's chat.
      </p>

      <div className="mt-5 space-y-3 rounded-xl border border-white/10 bg-[#141414] p-4">
        <div className="text-[11px] font-semibold uppercase tracking-wide text-zinc-500">From another app</div>
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <span className="text-zinc-500">I'm typing in</span>
          <Avatar id={from} size={18} />
          <select value={from} onChange={(e) => setFrom(e.target.value)} className={sel}>
            {askers.map((id) => (
              <option key={id} value={id}>
                {agentMeta(id).name}
              </option>
            ))}
          </select>
          <span className="text-zinc-500">and want</span>
          <Avatar id={target} size={18} />
          <select value={target} onChange={(e) => setTarget(e.target.value)} className={sel}>
            {targets.map((id) => (
              <option key={id} value={id}>
                {agentMeta(id).name}
              </option>
            ))}
          </select>
          <span className="text-zinc-500">to answer</span>
        </div>
        <input
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          placeholder={`what's the status of ${room.title}?`}
          className="w-full select-text rounded-md border border-white/10 bg-black/40 px-2.5 py-1.5 text-[13px] outline-none placeholder:text-zinc-600 focus:border-white/30"
        />
        <CopyBlock label={`Paste this into ${agentMeta(from).name}`} text={base} />
        {to.subtitle && <div className="text-[11px] text-zinc-600">{to.name} is a Grok bot, so its routine must be set up under Providers → Grok Bot.</div>}
      </div>

      <div className="mt-3 space-y-3 rounded-xl border border-white/10 bg-[#141414] p-4">
        <div className="text-[11px] font-semibold uppercase tracking-wide text-zinc-500">More ideas</div>
        {thenWho && (
          <CopyBlock
            label="Chain two agents: the answer is forwarded on"
            text={`${base}, with then="${thenWho}" so the answer is passed on to ${agentMeta(thenWho).name}.`}
          />
        )}
        <CopyBlock label="For agents that can't be woken (inbox only): paste into their chat" text="Check your AgentMux inbox with mux_inbox and answer anything waiting using mux_reply." />
        <CopyBlock label="See who you can reach" text="Use the agentmux tool mux_contacts and tell me who is connected." />
        {pool.length > 2 && (
          <div className="flex items-center gap-2 text-xs">
            <span className="text-zinc-500">Chain through</span>
            <select value={third} onChange={(e) => setThird(e.target.value)} className={sel}>
              <option value="">automatic</option>
              {pool.filter((id) => id !== from && id !== target).map((id) => (
                <option key={id} value={id}>
                  {agentMeta(id).name}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>

      <div className="mt-4 text-center text-xs text-zinc-500">
        Or ask from here: the box below sends as you.{" "}
        <button className="text-zinc-300 underline underline-offset-2" onClick={() => onPick(target)}>
          Address {to.name}
        </button>
      </div>
    </div>
  );
}
