import { useEffect, useState } from "react";
import { Check, Copy, Eye, EyeOff, RefreshCw } from "lucide-react";
import { agentMeta, api, onMuxEvent, type Gateway, type TunnelProvider } from "../lib/api";
import { Badge, Dot, btn, btnPrimary } from "./ui";

type Run = (fn: () => Promise<Gateway>) => Promise<void>;
const input =
  "min-w-0 flex-1 select-text rounded-md border border-white/10 bg-black/40 px-2 py-1.5 font-mono text-[11px] text-zinc-200 outline-none focus:border-white/30 disabled:opacity-60";

/** Permanent Cloudflare tunnel: connect (pick the domain in the browser), type a subdomain, done. */
function CloudflareSetup({ gw, busy, run }: { gw: Gateway; busy: boolean; live: boolean; run: Run }) {
  const [open, setOpen] = useState(false);
  const [sub, setSub] = useState("mux");
  const [token, setToken] = useState("");
  const [tokenHost, setTokenHost] = useState("");
  const [advanced, setAdvanced] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const [check, setCheck] = useState<{ ok: boolean; text: string } | null>(null);

  // Already logged in from before: look up which domain that login is for.
  useEffect(() => {
    if (gw.cfLoggedIn && !gw.cfZone && gw.cfMode === "quick") api.cfLogin().catch(() => {});
  }, [gw.cfLoggedIn, gw.cfZone, gw.cfMode]);

  const test = async () => {
    setCheck({ ok: true, text: "Testing… (new DNS records can take a minute)" });
    try {
      setCheck({ ok: true, text: await api.tunnelVerify() });
    } catch (e) {
      setCheck({ ok: false, text: String(e) });
    }
  };
  const login = async (fn: () => Promise<Gateway>) => {
    setWaiting(true);
    await run(fn);
    setWaiting(false);
  };

  if (gw.cfMode !== "quick") {
    return (
      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-2 text-zinc-400">
          <Badge tone="green">{gw.cfMode === "named" ? "Cloudflare tunnel" : "dashboard token"}</Badge>
          <span className="font-mono text-zinc-300">{gw.cfHostname}</span>
          <button className={`${btn} ml-auto`} disabled={gw.state !== "running"} onClick={test}>
            Test
          </button>
          <button className={btn} disabled={busy || gw.state === "running"} title={gw.state === "running" ? "Stop the tunnel first" : undefined} onClick={() => run(api.cfReset)}>
            Switch back to temporary
          </button>
        </div>
        {check && <div className={check.ok ? "text-emerald-300" : "text-red-300"}>{check.text}</div>}
      </div>
    );
  }
  if (!open) {
    return (
      <button className={btn} onClick={() => setOpen(true)}>
        Make it permanent with your domain on Cloudflare…
      </button>
    );
  }
  return (
    <div className="space-y-2.5 rounded-lg border border-white/10 p-3">
      <div className="font-semibold text-zinc-200">Permanent Cloudflare tunnel</div>
      <div className="text-zinc-500">Free, with any domain in your Cloudflare account. AgentMux creates the tunnel and its DNS record, starts it and tests it.</div>

      <div className="flex flex-wrap items-center gap-2">
        <span className="w-4 text-zinc-500">1</span>
        {gw.cfLoggedIn ? (
          <>
            <span className="flex items-center gap-1.5 text-emerald-300">
              <Check size={13} /> Cloudflare connected{gw.cfZone ? ` · ${gw.cfZone}` : ""}
            </span>
            <button className="text-zinc-500 underline underline-offset-2" disabled={waiting} onClick={() => login(api.cfSwitchDomain)}>
              {waiting ? "Waiting for your browser…" : "use a different domain"}
            </button>
          </>
        ) : (
          <>
            <button className={btnPrimary} disabled={busy || waiting} onClick={() => login(api.cfLogin)}>
              {waiting ? "Waiting for your browser…" : "Connect Cloudflare"}
            </button>
            <span className="text-zinc-500">Your browser opens: pick the domain and click Authorize.</span>
          </>
        )}
      </div>

      <div className="flex items-center gap-2">
        <span className="w-4 text-zinc-500">2</span>
        <div className={`flex min-w-0 flex-1 items-center rounded-md border border-white/10 bg-black/40 pr-2 ${gw.cfLoggedIn ? "" : "opacity-60"}`}>
          <input
            value={sub}
            onChange={(e) => setSub(e.target.value)}
            disabled={!gw.cfLoggedIn}
            spellCheck={false}
            placeholder="mux"
            className="min-w-0 flex-1 select-text bg-transparent px-2 py-1.5 font-mono text-[11px] text-zinc-200 outline-none"
          />
          {!sub.includes(".") && <span className="shrink-0 font-mono text-[11px] text-zinc-500">.{gw.cfZone ?? "‹the domain you authorized›"}</span>}
        </div>
        <button
          className={btnPrimary}
          disabled={busy || !gw.cfLoggedIn || !sub.trim()}
          onClick={async () => {
            await run(() => api.cfCreate(sub.trim()));
            await test();
          }}
        >
          {busy ? "Setting up…" : "Create & start"}
        </button>
      </div>
      <div className="pl-6 text-zinc-600">Type just the subdomain. Cloudflare doesn&apos;t tell apps which domain you authorized, so AgentMux learns it when it creates the record and shows the full address here.</div>
      {check && <div className={check.ok ? "pl-6 text-emerald-300" : "pl-6 text-red-300"}>{check.text}</div>}

      <button className="text-zinc-500 underline underline-offset-2" onClick={() => setAdvanced(!advanced)}>
        {advanced ? "Hide" : "Or use a tunnel token from the Cloudflare dashboard"}
      </button>
      {advanced && (
        <div className="space-y-2">
          <div className="text-zinc-500">
            In Zero Trust → Networks → Tunnels, create a tunnel, add a public hostname whose service is <span className="font-mono">http://localhost:{gw.port}</span>, then paste the token from its
            install command.
          </div>
          <input type="password" value={token} onChange={(e) => setToken(e.target.value)} placeholder="Tunnel token (eyJ…)" autoComplete="off" className={`${input} w-full`} />
          <div className="flex gap-2">
            <input value={tokenHost} onChange={(e) => setTokenHost(e.target.value)} placeholder="The public hostname you added, e.g. mux.yourdomain.com" spellCheck={false} className={input} />
            <button
              className={btnPrimary}
              disabled={busy || !token.trim() || !tokenHost.trim()}
              onClick={() => run(() => api.cfToken(token.trim(), tokenHost.trim())).then(() => setToken(""))}
            >
              Save
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/** ngrok: authtoken (keychain) plus a static domain makes the URL permanent. */
function NgrokSetup({ gw, busy, live, run }: { gw: Gateway; busy: boolean; live: boolean; run: Run }) {
  const [token, setToken] = useState("");
  const [domain, setDomain] = useState(gw.ngrokDomain ?? "");
  const dirty = token.trim() !== "" || domain.trim() !== (gw.ngrokDomain ?? "");
  return (
    <div className="space-y-2 rounded-lg border border-white/10 p-3">
      <div className="font-semibold text-zinc-200">ngrok</div>
      <div className="text-zinc-500">
        Free accounts get one static domain (dashboard.ngrok.com → Domains). Paste it here with your authtoken to keep the same URL. Without a domain the URL is temporary.
      </div>
      <input
        type="password"
        value={token}
        disabled={live}
        onChange={(e) => setToken(e.target.value)}
        placeholder={gw.ngrokAuthtoken ? "Authtoken saved in Keychain (paste to replace)" : "Authtoken (optional if ngrok is already configured)"}
        autoComplete="off"
        className={`${input} w-full`}
      />
      <div className="flex gap-2">
        <input value={domain} disabled={live} onChange={(e) => setDomain(e.target.value)} placeholder="Static domain, e.g. my-app.ngrok-free.app" spellCheck={false} className={input} />
        <button
          className={btnPrimary}
          disabled={busy || live || !dirty}
          onClick={() => run(() => api.ngrokSetup(token.trim() || null, domain.trim())).then(() => setToken(""))}
        >
          Save
        </button>
      </div>
    </div>
  );
}

const LABEL: Record<TunnelProvider, string> = { cloudflared: "Cloudflare Tunnel", ngrok: "ngrok" };
const INSTALL: Record<TunnelProvider, string> = { cloudflared: "brew install cloudflared", ngrok: "brew install ngrok" };

/** Starts/stops the tunnel that lets cloud agents (Grok) reach this Mac, and shows what to paste into them. */
export default function TunnelPanel({ compact, agent = "grok" }: { compact?: boolean; agent?: string }) {
  const [gw, setGw] = useState<Gateway | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [show, setShow] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  // Each bot has its own token; never show one bot's token under another.
  useEffect(() => {
    setToken(null);
    setShow(false);
  }, [agent]);

  useEffect(() => {
    api.gatewayStatus().then(setGw).catch((e) => setErr(String(e)));
    const off = onMuxEvent((e) => e.kind === "tunnel" && setGw(e.data as Gateway));
    return () => void off.then((f) => f());
  }, []);

  const copy = (what: string, value: string) =>
    navigator.clipboard?.writeText(value).then(() => {
      setCopied(what);
      setTimeout(() => setCopied(null), 1500);
    });

  const run = async (fn: () => Promise<Gateway>) => {
    setBusy(true);
    setErr(null);
    try {
      setGw(await fn());
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const reveal = async (rotate = false) => {
    setErr(null);
    try {
      const r = await (rotate ? api.gatewayRotate(agent) : api.gatewayReveal(agent));
      setToken(r.token);
      setShow(true);
    } catch (e) {
      setErr(String(e));
    }
  };

  if (!gw) return <div className="text-xs text-zinc-500">{err ?? "Loading…"}</div>;
  const live = gw.state === "starting" || gw.state === "running";
  const tone = gw.state === "running" ? "green" : gw.state === "error" ? "red" : gw.state === "starting" ? "amber" : "zinc";

  return (
    <div className="space-y-3 text-xs">
      {!compact && (
        <div className="text-zinc-400">
          Cloud agents like Grok can&apos;t reach <span className="font-mono">localhost</span>. A tunnel publishes AgentMux&apos;s HTTP endpoint over HTTPS while it runs. It is off by
          default, stops when you quit AgentMux, and the token only unlocks <span className="font-mono">{gw.tools.join(", ")}</span>.
        </div>
      )}

      <div className="flex flex-wrap gap-2">
        {(["cloudflared", "ngrok"] as TunnelProvider[]).map((p) => (
          <button
            key={p}
            disabled={busy || live}
            onClick={() => run(() => api.gatewaySetProvider(p))}
            className={`flex items-center gap-2 rounded-lg border px-3 py-2 text-left disabled:opacity-60 ${gw.provider === p ? "border-white/30 bg-white/10" : "border-white/10 hover:bg-white/5"}`}
          >
            <span className="font-medium">{LABEL[p]}</span>
            {p === "cloudflared" && <Badge>default</Badge>}
            <Badge tone={gw.installed[p] ? "green" : "amber"}>{gw.installed[p] ? "installed" : "not installed"}</Badge>
          </button>
        ))}
      </div>

      {gw.permanent && !gw.verified ? (
        <div className="rounded-md bg-amber-500/10 p-2 text-amber-200">
          <b>Not verified yet.</b> A permanent tunnel is set up for <span className="font-mono">https://{gw.provider === "ngrok" ? gw.ngrokDomain : gw.cfHostname}</span>, but it hasn&apos;t been
          confirmed to reach this Mac from the internet. Start it and click <b>Test</b>. Don&apos;t paste the URL into Grok until the test passes.
        </div>
      ) : gw.permanent ? (
        <div className="flex flex-wrap items-center gap-2 rounded-md bg-emerald-500/10 p-2 text-emerald-200">
          <span className="min-w-0 flex-1">
            Permanent URL <span className="font-mono">https://{gw.provider === "ngrok" ? gw.ngrokDomain : gw.cfHostname}</span>. It survives restarts, so each Grok bot&apos;s connector only needs it once.
          </span>
          <label className="flex cursor-pointer items-center gap-1.5">
            <input type="checkbox" checked={gw.autostart} disabled={busy} onChange={(e) => run(() => api.setAutostart(e.target.checked))} />
            Start when AgentMux opens
          </label>
        </div>
      ) : (
        <div className="rounded-md bg-amber-500/10 p-2 text-amber-200">
          <b>Temporary URL.</b> A quick tunnel gets a new random address every time it or AgentMux restarts, and you&apos;d paste it into every Grok bot again. To keep one URL,
          connect your own domain through Cloudflare or a static ngrok domain below.
        </div>
      )}

      {gw.provider === "cloudflared" && <CloudflareSetup gw={gw} busy={busy} live={live} run={run} />}
      {gw.provider === "ngrok" && <NgrokSetup gw={gw} busy={busy} live={live} run={run} />}

      {!gw.installed[gw.provider] && (
        <div className="flex items-center gap-2 rounded bg-black/40 p-2 font-mono text-[11px] text-zinc-200">
          <span className="min-w-0 flex-1">{INSTALL[gw.provider]}</span>
          <button onClick={() => copy("install", INSTALL[gw.provider])} className="shrink-0 text-zinc-400 hover:text-white">
            {copied === "install" ? <Check size={14} /> : <Copy size={14} />}
          </button>
        </div>
      )}

      <div className="flex flex-wrap items-center gap-2">
        {live ? (
          <button className={btn} disabled={busy} onClick={() => run(api.gatewayStop)}>
            Stop tunnel
          </button>
        ) : (
          <button className={btnPrimary} disabled={busy || !gw.installed[gw.provider] || !gw.http} onClick={() => run(api.gatewayStart)}>
            Start tunnel
          </button>
        )}
        <Dot on={gw.state === "running"} pulse={gw.state === "starting"} />
        <Badge tone={tone}>{gw.state}</Badge>
        {!gw.http && <span className="text-amber-300">local endpoint not running (port {gw.port} busy?)</span>}
      </div>

      {gw.error && <pre className="whitespace-pre-wrap rounded-md bg-red-500/10 p-2 font-mono text-[11px] text-red-200">{gw.error}</pre>}
      {err && <pre className="whitespace-pre-wrap rounded-md bg-red-500/10 p-2 font-mono text-[11px] text-red-200">{err}</pre>}

      {gw.state === "running" && gw.mcpUrl && (
        <div className="space-y-2 rounded-lg border border-white/10 p-3">
          <div>
            <div className="mb-1 text-zinc-500">MCP server URL</div>
            <div className="flex items-center gap-2 rounded bg-black/40 p-2 font-mono text-[11px] text-zinc-200">
              <span className="min-w-0 flex-1 select-text break-all">{gw.mcpUrl}</span>
              <button onClick={() => copy("url", gw.mcpUrl!)} className="shrink-0 text-zinc-400 hover:text-white">
                {copied === "url" ? <Check size={14} /> : <Copy size={14} />}
              </button>
            </div>
          </div>
          <div>
            <div className="mb-1 text-zinc-500">Authorization: Bearer token for {agentMeta(agent).name}</div>
            <div className="flex items-center gap-2 rounded bg-black/40 p-2 font-mono text-[11px] text-zinc-200">
              <span className="min-w-0 flex-1 select-text break-all">{token ? (show ? token : "•".repeat(24)) : "stored in the macOS Keychain"}</span>
              {token ? (
                <>
                  <button onClick={() => setShow(!show)} className="shrink-0 text-zinc-400 hover:text-white" title={show ? "Hide" : "Show"}>
                    {show ? <EyeOff size={14} /> : <Eye size={14} />}
                  </button>
                  <button onClick={() => copy("token", token)} className="shrink-0 text-zinc-400 hover:text-white" title="Copy">
                    {copied === "token" ? <Check size={14} /> : <Copy size={14} />}
                  </button>
                </>
              ) : (
                <button className={btn} onClick={() => reveal()}>
                  Reveal
                </button>
              )}
              <button className="shrink-0 text-zinc-400 hover:text-white" title="Rotate token (locks out the old one)" onClick={() => reveal(true)}>
                <RefreshCw size={14} />
              </button>
            </div>
          </div>
          {!gw.permanent && <div className="text-zinc-500">This URL is temporary: it changes every time the tunnel starts. Update each Grok bot&apos;s connector when it does.</div>}
        </div>
      )}
    </div>
  );
}
