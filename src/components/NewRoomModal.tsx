import { useState } from "react";
import { FolderOpen } from "lucide-react";
import { agentMeta, api, pickFolder, type Overview } from "../lib/api";
import { Avatar, Dot, Modal, btn, btnPrimary } from "./ui";

/** Create a bridge room: choose the folder and which providers take part. */
export default function NewRoomModal({
  ov,
  onClose,
  onCreated,
}: {
  ov: Overview;
  onClose: () => void;
  onCreated: (roomId: string) => void;
}) {
  const [path, setPath] = useState("");
  const [picked, setPicked] = useState<Set<string>>(
    new Set(ov.providers.filter((p) => p.connectedAt).map((p) => p.id)),
  );
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const toggle = (id: string) =>
    setPicked((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });

  const choose = async () => {
    try {
      const p = await pickFolder();
      if (p) setPath(p);
    } catch (e) {
      setErr(String(e));
    }
  };

  const create = async () => {
    setBusy(true);
    setErr(null);
    try {
      onCreated(await api.createRoom(path.trim(), [...picked]));
    } catch (e) {
      setErr(String(e));
      setBusy(false);
    }
  };

  return (
    <Modal title="New bridge room" onClose={onClose}>
      <div className="space-y-5 text-sm">
        <div className="text-xs leading-relaxed text-zinc-400">
          A room is one workspace (a repo or folder) that your agents share: its
          git diff, notes and every question they ask each other.
        </div>

        <div>
          <div className="mb-1.5 text-xs font-semibold text-zinc-300">
            1 · Workspace folder
          </div>
          <div className="flex gap-2">
            <input
              autoFocus
              value={path}
              onChange={(e) => setPath(e.target.value)}
              onKeyDown={(e) =>
                e.key === "Enter" && path.trim() && !busy && create()
              }
              placeholder="/path/to/repo"
              spellCheck={false}
              className="min-w-0 flex-1 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1.5 font-mono text-[11px] outline-none focus:border-white/30"
            />
            <button className={btn} onClick={choose}>
              <FolderOpen size={13} /> Choose…
            </button>
          </div>
        </div>

        <div>
          <div className="mb-1.5 text-xs font-semibold text-zinc-300">
            2 · Who takes part
          </div>
          <div className="space-y-1">
            {ov.providers.map((p) => (
              <label
                key={p.id}
                className="flex cursor-pointer items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-white/5"
              >
                <input
                  type="checkbox"
                  checked={picked.has(p.id)}
                  onChange={() => toggle(p.id)}
                />
                <Avatar id={p.id} size={22} />
                <span className="flex-1 text-[13px]">
                  {agentMeta(p.id).name}
                </span>
                <Dot on={!!p.connectedAt} />
                <span className="w-24 text-right text-[11px] text-zinc-500">
                  {p.connectedAt ? "connected" : "not connected"}
                </span>
              </label>
            ))}
          </div>
          <div className="mt-1 px-2 text-[11px] text-zinc-600">
            Connected providers are pre-selected. Others can join later when
            they first use the room.
          </div>
        </div>

        {err && (
          <pre className="whitespace-pre-wrap rounded-md bg-red-500/10 p-2 font-mono text-[11px] text-red-200">
            {err}
          </pre>
        )}
        <div className="flex justify-end gap-2">
          <button className={btn} onClick={onClose}>
            Cancel
          </button>
          <button
            className={btnPrimary}
            disabled={busy || !path.trim()}
            onClick={create}
          >
            Create room
          </button>
        </div>
      </div>
    </Modal>
  );
}
