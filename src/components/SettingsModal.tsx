import { useEffect, useState } from "react";
import { api, type Overview, type VaultFile } from "../lib/api";
import { Badge, Modal, btn, btnPrimary } from "./ui";
import SyncPanel from "./SyncPanel";
import EnginesPanel from "./EnginesPanel";
import TunnelPanel from "./TunnelPanel";
import { UpdateSettings } from "./UpdateControls";

export type Section = "daemon" | "access" | "engines" | "sync" | "vault" | "inspector";

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex gap-4 border-b border-white/5 py-2 text-xs">
      <div className="w-32 shrink-0 text-zinc-500">{k}</div>
      <div className="min-w-0 break-all font-mono text-zinc-200">{v}</div>
    </div>
  );
}

function Vault() {
  const [files, setFiles] = useState<VaultFile[]>([]);
  const [sel, setSel] = useState<string>("");
  const [text, setText] = useState("");
  const [saved, setSaved] = useState("");
  const load = () =>
    api.vaultFiles().then((f) => {
      setFiles(f);
      if (!sel && f[0]) {
        setSel(f[0].filename);
        setText(f[0].content);
      }
    });
  useEffect(() => {
    load().catch(() => {});
  }, []);
  if (!files.length) return <div className="text-xs text-zinc-500">No thread files yet. They appear in ~/.agentmux/threads once agents talk.</div>;
  return (
    <div className="flex gap-4">
      <div className="w-52 shrink-0 space-y-1">
        {files.map((f) => (
          <button
            key={f.filename}
            onClick={() => {
              setSel(f.filename);
              setText(f.content);
              setSaved("");
            }}
            className={`block w-full truncate rounded px-2 py-1.5 text-left font-mono text-[11px] ${sel === f.filename ? "bg-white/10" : "hover:bg-white/5"}`}
          >
            {f.filename}
          </button>
        ))}
      </div>
      <div className="min-w-0 flex-1">
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          spellCheck={false}
          className="h-80 w-full resize-none select-text rounded-md border border-white/10 bg-black/40 p-3 font-mono text-[11px] text-zinc-200 outline-none focus:border-white/30"
        />
        <div className="mt-2 flex items-center gap-3">
          <button
            className={btnPrimary}
            onClick={async () => {
              await api.saveVaultFile(sel, text);
              setSaved("Saved atomically (0600)");
              load();
            }}
          >
            Save
          </button>
          <span className="text-xs text-zinc-500">{saved}</span>
        </div>
      </div>
    </div>
  );
}

function Inspector() {
  const [tools, setTools] = useState<any[]>([]);
  const [tool, setTool] = useState("mux_contacts");
  const [args, setArgs] = useState("{}");
  const [out, setOut] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api
      .mcp({ jsonrpc: "2.0", id: 1, method: "tools/list" })
      .then((r) => setTools(r?.result?.tools ?? []))
      .catch(() => {});
  }, []);
  const schema = tools.find((t) => t.name === tool)?.inputSchema;
  return (
    <div className="space-y-3 text-xs">
      <div className="text-zinc-500">Calls the in-process MCP server as the human user — the same code path the agents hit.</div>
      <select
        value={tool}
        onChange={(e) => {
          setTool(e.target.value);
          setArgs("{}");
        }}
        className="w-full rounded-md border border-white/10 bg-black/40 p-2 font-mono"
      >
        {tools.map((t) => (
          <option key={t.name} value={t.name}>
            {t.name}
          </option>
        ))}
      </select>
      {schema && <pre className="select-text overflow-x-auto rounded bg-black/40 p-2 font-mono text-[10px] text-zinc-500">{JSON.stringify(schema.properties)}</pre>}
      <textarea
        value={args}
        onChange={(e) => setArgs(e.target.value)}
        spellCheck={false}
        className="h-20 w-full select-text rounded-md border border-white/10 bg-black/40 p-2 font-mono outline-none focus:border-white/30"
      />
      <button
        className={btnPrimary}
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            const r = await api.mcp({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: tool, arguments: JSON.parse(args || "{}") } });
            setOut(r?.result?.content?.[0]?.text ?? JSON.stringify(r, null, 2));
          } catch (e) {
            setOut(`error: ${e}`);
          } finally {
            setBusy(false);
          }
        }}
      >
        {busy ? "Running…" : "Run tool"}
      </button>
      {out && <pre className="max-h-64 select-text overflow-auto whitespace-pre-wrap rounded bg-black/40 p-3 font-mono text-[11px] text-zinc-200">{out}</pre>}
    </div>
  );
}

export default function SettingsModal({ ov, onClose, initial = "daemon" }: { ov: Overview; onClose: () => void; initial?: Section }) {
  const [sec, setSec] = useState<Section>(initial);
  useEffect(() => setSec(initial), [initial]);
  const tabs: [Section, string][] = [
    ["daemon", "Daemon"],
    ["access", "Public access"],
    ["engines", "Engines"],
    ["sync", "Sync"],
    ["vault", "Vault"],
    ["inspector", "MCP Inspector"],
  ];
  return (
    <Modal title="Settings" onClose={onClose} wide>
      <div className="mb-4 flex gap-1">
        {tabs.map(([id, label]) => (
          <button key={id} onClick={() => setSec(id)} className={`rounded-md px-3 py-1.5 text-xs ${sec === id ? "bg-white/10" : "text-zinc-400 hover:bg-white/5"}`}>
            {label}
          </button>
        ))}
      </div>
      {sec === "daemon" && (
        <div>
          <div className="mb-3 flex items-center gap-2 text-xs">
            Daemon <Badge tone={ov.daemon.startsWith("error") ? "red" : ov.daemon === "off" ? "zinc" : "green"}>{ov.daemon}</Badge>
          </div>
          <Row k="Transport" v="Unix socket (0600) — one stdio shim per agent" />
          <Row k="Socket" v={ov.socket} />
          <Row k="Vault" v={ov.home} />
          <Row k="Shim command" v={`${ov.home}/bin/agentmux mcp --agent <id>`} />
          <Row k="HTTP gateway" v="127.0.0.1:4317/mcp (bearer token; only reachable from outside through a tunnel you start)" />
          <Row k="Wake safety" v="read-only by default; per-room toggle allows edits; hop limit 3; 5 min timeout" />
          <UpdateSettings />
        </div>
      )}
      {sec === "access" && <TunnelPanel />}
      {sec === "engines" && <EnginesPanel />}
      {sec === "sync" && <SyncPanel />}
      {sec === "vault" && <Vault />}
      {sec === "inspector" && <Inspector />}
    </Modal>
  );
}
