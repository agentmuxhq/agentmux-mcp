import { useEffect, useState } from "react";
import { Check, Copy } from "lucide-react";
import { api, onMuxEvent, type Gateway } from "../lib/api";
import { Badge, Dot } from "./ui";

/** Status summary of the tunnel that lets cloud agents (Grok) reach this Mac. */
export default function PublicAccessCard({
  onManage,
}: {
  onManage: () => void;
}) {
  const [gw, setGw] = useState<Gateway | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    api
      .gatewayStatus()
      .then(setGw)
      .catch(() => {});
    const off = onMuxEvent(
      (e) => e.kind === "tunnel" && setGw(e.data as Gateway),
    );
    return () => void off.then((f) => f());
  }, []);
  if (!gw) return null;

  const running = gw.state === "running";
  const host =
    gw.url?.replace("https://", "") ??
    (gw.provider === "ngrok" ? gw.ngrokDomain : gw.cfHostname);
  const kind = gw.provider === "ngrok" ? "ngrok" : "Cloudflare";
  const mode = gw.permanent
    ? gw.verified
      ? "permanent · verified"
      : "permanent · not verified"
    : "temporary";
  const tone =
    gw.state === "error"
      ? "red"
      : running
        ? gw.permanent && !gw.verified
          ? "amber"
          : "green"
        : gw.state === "starting"
          ? "amber"
          : "zinc";

  return (
    <div className="rounded-lg border border-white/10 p-3">
      <div className="mb-2 flex items-center gap-2">
        <Dot on={running} pulse={gw.state === "starting"} />
        <span className="font-display text-sm font-semibold">
          Public access
        </span>
        <Badge tone={tone}>
          {running ? "online" : gw.state === "stopped" ? "off" : gw.state}
        </Badge>
        <button
          onClick={onManage}
          className="ml-auto text-[11px] text-zinc-500 underline underline-offset-2 hover:text-zinc-300"
        >
          manage
        </button>
      </div>
      <div className="space-y-1 text-[11px]">
        <Row k="Proxy" v={`${kind} tunnel · ${mode}`} />
        {host && <Row k="Domain" v={host} mono />}
        {gw.mcpUrl && (
          <div className="flex items-center gap-2">
            <span className="w-14 shrink-0 text-zinc-500">MCP</span>
            <span
              className="min-w-0 flex-1 select-text truncate font-mono text-zinc-300"
              title={gw.mcpUrl}
            >
              {gw.mcpUrl}
            </span>
            <button
              title="Copy"
              className="shrink-0 text-zinc-500 hover:text-white"
              onClick={() =>
                navigator.clipboard?.writeText(gw.mcpUrl!).then(() => {
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1500);
                })
              }
            >
              {copied ? <Check size={12} /> : <Copy size={12} />}
            </button>
          </div>
        )}
        <Row
          k="Local"
          v={`http://${gw.http ?? `127.0.0.1:${gw.port}`}/mcp`}
          mono
        />
        {gw.error && <div className="text-red-300">{gw.error}</div>}
        {running && !gw.permanent && (
          <div className="text-amber-300/80">
            Temporary URL: it changes when the tunnel restarts.
          </div>
        )}
      </div>
    </div>
  );
}

function Row({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="flex gap-2">
      <span className="w-14 shrink-0 text-zinc-500">{k}</span>
      <span
        className={`min-w-0 flex-1 truncate ${mono ? "font-mono" : ""} text-zinc-300`}
        title={v}
      >
        {v}
      </span>
    </div>
  );
}
