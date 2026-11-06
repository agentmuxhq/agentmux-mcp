import { useMemo, useState } from "react";
import { ago, api, type Overview } from "../lib/api";
import { Avatar, Modal, btn } from "./ui";

const DAY = 24 * 60 * 60 * 1000;

/** Pick old or junk rooms and delete them for good. Oldest first; nothing is selected until you choose. */
export default function CleanupRoomsModal({
  ov,
  onClose,
  onChanged,
}: {
  ov: Overview;
  onClose: () => void;
  onChanged: () => void;
}) {
  const rooms = useMemo(
    () => [...ov.rooms].sort((a, b) => a.updatedAt.localeCompare(b.updatedAt)),
    [ov.rooms],
  );
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  const toggle = (id: string) =>
    setPicked((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });
  const olderThan = (days: number) =>
    setPicked(
      new Set(
        rooms
          .filter((r) => Date.now() - Date.parse(r.updatedAt) > days * DAY)
          .map((r) => r.id),
      ),
    );

  const del = async () => {
    const chosen = rooms.filter((r) => picked.has(r.id));
    if (!chosen.length) return;
    const list = chosen
      .map((r) => `• ${r.title} (${r.sessions.length} threads)`)
      .join("\n");
    if (
      !window.confirm(
        `Delete ${chosen.length} room${chosen.length > 1 ? "s" : ""} for good?\n\n${list}\n\nTheir threads, messages, questions and attachments are erased and can't be brought back. To keep the history, use "Merge into…" instead.`,
      )
    )
      return;
    setBusy(true);
    setMsg(null);
    const failed: string[] = [];
    for (const r of chosen) {
      await api.deleteRoom(r.id).catch((e) => failed.push(`${r.title}: ${e}`));
    }
    setBusy(false);
    setPicked(new Set());
    onChanged();
    setMsg(
      failed.length
        ? failed.join("\n")
        : `Deleted ${chosen.length} room${chosen.length > 1 ? "s" : ""}.`,
    );
  };

  return (
    <Modal title="Clean up rooms" onClose={onClose}>
      <div className="space-y-3 text-xs">
        <div className="text-zinc-500">
          Pick rooms to delete for good, oldest first. Rooms that belong to the
          same product are better merged (open the room, then{" "}
          <span className="text-zinc-300">Merge into…</span>), which keeps their
          history.
        </div>
        <div className="flex flex-wrap gap-2">
          <button className={btn} onClick={() => olderThan(30)}>
            Not used in 30 days
          </button>
          <button className={btn} onClick={() => olderThan(7)}>
            Not used in 7 days
          </button>
          <button className={btn} onClick={() => setPicked(new Set())}>
            Clear selection
          </button>
        </div>
        <div className="max-h-[50vh] space-y-1 overflow-y-auto">
          {rooms.map((r) => (
            <label
              key={r.id}
              className={`flex cursor-pointer items-center gap-2.5 rounded-md border px-2.5 py-2 ${picked.has(r.id) ? "border-red-500/40 bg-red-500/5" : "border-white/10 hover:bg-white/5"}`}
            >
              <input
                type="checkbox"
                checked={picked.has(r.id)}
                onChange={() => toggle(r.id)}
              />
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px] text-zinc-200">
                  {r.title}
                </div>
                <div className="truncate text-[11px] text-zinc-500">
                  {r.sessions.length} thread{r.sessions.length === 1 ? "" : "s"}{" "}
                  · last used {ago(r.updatedAt)} ·{" "}
                  <span className="font-mono">{r.workspacePath}</span>
                </div>
              </div>
              <div className="flex -space-x-1.5">
                {r.participants.slice(0, 3).map((p) => (
                  <div key={p} className="rounded-full ring-2 ring-[#161616]">
                    <Avatar id={p} size={16} />
                  </div>
                ))}
              </div>
            </label>
          ))}
        </div>
        {msg && (
          <pre className="whitespace-pre-wrap rounded-md bg-white/5 p-2 font-mono text-[11px] text-zinc-300">
            {msg}
          </pre>
        )}
        <div className="flex justify-end gap-2">
          <button className={btn} onClick={onClose}>
            Close
          </button>
          <button
            className="rounded-md bg-red-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-red-500 disabled:opacity-40"
            disabled={busy || picked.size === 0}
            onClick={del}
          >
            {busy
              ? "Deleting…"
              : `Delete ${picked.size || ""} room${picked.size === 1 ? "" : "s"}`}
          </button>
        </div>
      </div>
    </Modal>
  );
}
