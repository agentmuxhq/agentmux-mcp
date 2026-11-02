import type { ReactNode } from "react";
import { X } from "lucide-react";
import { agentMeta } from "../lib/api";

export function Avatar({ id, size = 28 }: { id: string; size?: number }) {
  const meta = id === "user" || id === "agentmux" ? null : agentMeta(id);
  const color = meta?.color ?? "#6B7280";
  const label = id === "user" ? "You" : meta ? meta.name[0]?.toUpperCase() : id === "agentmux" ? "A" : id[0]?.toUpperCase();
  return (
    <div
      className="flex shrink-0 items-center justify-center rounded-full font-display font-semibold text-white"
      style={{ width: size, height: size, background: color, fontSize: size * 0.4 }}
      title={meta?.name ?? id}
    >
      {label}
    </div>
  );
}

export function Dot({ on, pulse }: { on: boolean; pulse?: boolean }) {
  return (
    <span className="relative flex h-2 w-2">
      {pulse && <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-amber-400 opacity-60" />}
      <span className={`relative inline-flex h-2 w-2 rounded-full ${on ? "bg-emerald-400" : pulse ? "bg-amber-400" : "bg-zinc-600"}`} />
    </span>
  );
}

export function Badge({ children, tone = "zinc" }: { children: ReactNode; tone?: "zinc" | "green" | "amber" | "red" }) {
  const tones = {
    zinc: "bg-white/5 text-zinc-400",
    green: "bg-emerald-500/10 text-emerald-300",
    amber: "bg-amber-500/10 text-amber-300",
    red: "bg-red-500/10 text-red-300",
  };
  return <span className={`rounded px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide ${tones[tone]}`}>{children}</span>;
}

export function Modal({ title, onClose, children, wide }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" onMouseDown={onClose}>
      <div
        className={`flex max-h-[86vh] w-full flex-col rounded-xl border border-white/10 bg-[#161616] shadow-2xl ${wide ? "max-w-4xl" : "max-w-xl"}`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-white/10 px-5 py-3">
          <h2 className="font-display text-sm font-semibold">{title}</h2>
          <button onClick={onClose} className="rounded p-1 text-zinc-400 hover:bg-white/10 hover:text-white">
            <X size={16} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-5">{children}</div>
      </div>
    </div>
  );
}

export const btn =
  "inline-flex items-center gap-1.5 rounded-md border border-white/10 bg-white/5 px-3 py-1.5 text-xs font-medium text-zinc-200 hover:bg-white/10 disabled:opacity-40 disabled:hover:bg-white/5";
export const btnPrimary =
  "inline-flex items-center gap-1.5 rounded-md bg-white px-3 py-1.5 text-xs font-semibold text-black hover:bg-zinc-200 disabled:opacity-40";
