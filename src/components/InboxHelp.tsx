import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { agentMeta, type Overview } from "../lib/api";
import { Avatar } from "./ui";

/** Questions waiting on agents nothing can wake (Claude Desktop, a Grok bot without a webhook).
 *  Instead of a spinner that never ends: say so, and hand the user a prompt to paste there. */
export default function InboxHelp({
  ov,
  waiting,
}: {
  ov: Overview;
  waiting: [string, { from: string; to: string }][];
}) {
  const [copied, setCopied] = useState<string | null>(null);
  const inboxOnly = (id: string) =>
    ov.providers.find((p) => p.id === id)?.wakeKind === "inbox";
  const byAgent = new Map<string, { ids: string[]; from: Set<string> }>();
  for (const [id, t] of waiting) {
    if (!inboxOnly(t.to)) continue;
    const g = byAgent.get(t.to) ?? { ids: [], from: new Set<string>() };
    g.ids.push(id);
    g.from.add(t.from === "user" ? "you" : agentMeta(t.from).name);
    byAgent.set(t.to, g);
  }
  if (byAgent.size === 0) return null;

  return (
    <div className="space-y-2 border-t border-white/10 px-6 py-3">
      {[...byAgent.entries()].map(([agent, g]) => {
        const name = agentMeta(agent).name;
        const app =
          agent === "claude-desktop"
            ? "the Claude app"
            : agent.startsWith("grok")
              ? "Grok"
              : name;
        const n = g.ids.length;
        const from = [...g.from].join(", ");
        const prompt =
          `Check AgentMux: you have ${n} question${n > 1 ? "s" : ""} waiting from ${from} (${g.ids.join(", ")}). ` +
          `Call mux_inbox to read them, then answer each one with mux_reply and its ticket id. ` +
          `If an answer needs something only this chat has (a script, files, decisions), include it in full, or save it where the question asks.`;
        return (
          <div
            key={agent}
            className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-3 text-xs"
          >
            <div className="mb-1.5 flex items-center gap-2 font-semibold text-amber-200">
              <Avatar id={agent} size={16} />
              {n} question{n > 1 ? "s" : ""} waiting for {name}
            </div>
            <div className="mb-2 leading-relaxed text-zinc-400">
              {name} can&apos;t be woken automatically: it only reads AgentMux
              when you use it. Open {app}, and paste this into a chat that has
              AgentMux. When it answers, AgentMux passes the reply to {from} on
              its own
              {from === "you"
                ? ""
                : `, and ${from} was told it's waiting on you`}
              .
            </div>
            <div className="flex items-start gap-2 rounded bg-black/40 p-2 text-[11px] leading-relaxed text-zinc-300">
              <span className="min-w-0 flex-1 select-text">{prompt}</span>
              <button
                title="Copy"
                className="shrink-0 text-zinc-400 hover:text-white"
                onClick={() =>
                  navigator.clipboard?.writeText(prompt).then(() => {
                    setCopied(agent);
                    setTimeout(() => setCopied(null), 1500);
                  })
                }
              >
                {copied === agent ? <Check size={14} /> : <Copy size={14} />}
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
