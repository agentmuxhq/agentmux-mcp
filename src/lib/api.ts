import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

/** `cursor`, `claude`, `chatgpt`, `grok`, or an added bot such as `grok:job-hunter`. */
export type AgentId = string;
export type FixedAgent = "cursor" | "grok" | "claude" | "chatgpt";

export const AGENTS: Record<FixedAgent, { name: string; handle: string; color: string; hint: string }> = {
  cursor: { name: "Cursor", handle: "@cursor", color: "#3B82F6", hint: "MCP stdio · ~/.cursor/mcp.json" },
  grok: { name: "Grok Bot", handle: "@grok", color: "#D97736", hint: "MCP stdio · added in Grok Bot settings" },
  claude: { name: "Claude Code", handle: "@claude", color: "#10B981", hint: "MCP stdio · claude mcp add" },
  chatgpt: { name: "ChatGPT", handle: "@chatgpt", color: "#8B5CF6", hint: "MCP stdio · ~/.codex/config.toml" },
};
export const AGENT_IDS = Object.keys(AGENTS) as FixedAgent[];

export const isGrok = (id: string) => id === "grok" || id.startsWith("grok:");

let known: Provider[] = [];
/** Called whenever the overview refreshes so names/colours of added bots resolve everywhere. */
export const setKnownProviders = (ps: Provider[]) => {
  known = ps;
};

/** Display info for any agent id, including bots and their self-registered names. */
export function agentMeta(id: string): { name: string; handle: string; color: string; subtitle: string | null } {
  const p = known.find((x) => x.id === id);
  const fixed = AGENTS[id as FixedAgent];
  const family = isGrok(id) ? AGENTS.grok : fixed;
  const name = p?.name ?? fixed?.name ?? id;
  return {
    name,
    handle: `@${id}`,
    color: family?.color ?? "#6B7280",
    subtitle: isGrok(id) && name !== AGENTS.grok.name ? "Grok Bot" : null,
  };
}

export interface Provider {
  id: AgentId;
  name: string;
  wakeKind: "cli" | "inbox" | "webhook" | null;
  connectedAt: string | null;
  lastSeen: string | null;
  calls: number;
  description: string | null;
  family: string | null;
  registered: boolean;
  canAsk: boolean;
}
export interface Session {
  id: string;
  title: string;
  status: string;
  startedAt: string;
}
export interface Room {
  id: string;
  title: string;
  kind: string;
  workspacePath: string;
  branch: string | null;
  allowEdits: boolean;
  updatedAt: string;
  participants: AgentId[];
  sessions: Session[];
  context: { notes?: string; terminal?: string; diff?: string };
}
export interface Overview {
  daemon: string;
  socket: string;
  home: string;
  providers: Provider[];
  rooms: Room[];
}
export interface Message {
  id: string;
  sender: string;
  recipient: string | null;
  type: string;
  text: string;
  codeBlock: string | null;
  ticketId: string | null;
  createdAt: string;
}
export interface Detect {
  agent: AgentId;
  installed: boolean;
  configured: boolean;
  detail: string;
}
export interface VaultFile {
  filename: string;
  diskPath: string;
  sizeBytes: number;
  content: string;
}
export interface GrokWebhook {
  agent: string;
  configured: boolean;
  url: string | null;
  hasKey: boolean;
  instructions: string;
}
export type TunnelProvider = "cloudflared" | "ngrok";
export interface Gateway {
  provider: TunnelProvider;
  ngrokDomain: string | null;
  ngrokAuthtoken: boolean;
  cfMode: "quick" | "named" | "token";
  cfHostname: string | null;
  cfZone: string | null;
  cfLoggedIn: boolean;
  permanent: boolean;
  verified: boolean;
  autostart: boolean;
  installed: Record<TunnelProvider, boolean>;
  state: "stopped" | "starting" | "running" | "error";
  url: string | null;
  mcpUrl: string | null;
  error: string | null;
  port: number;
  http: string | null;
  tools: string[];
}
export interface MuxEvent {
  kind: "message" | "ticket" | "provider_connected" | "room" | "context" | "tunnel";
  data: any;
}

export const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const call = <T>(cmd: string, args?: Record<string, unknown>) => invoke<T>(cmd, args);

export const api = {
  overview: () => call<Overview>("get_overview"),
  detect: () => call<Detect[]>("detect_providers"),
  connect: (id: string) => call<string>("connect_provider", { id }),
  disconnect: (id: string) => call<string>("disconnect_provider", { id }),
  messages: (sessionId: string) => call<Message[]>("get_messages", { sessionId }),
  send: (room: string, session: string | null, to: string, text: string, then?: string) =>
    call<string>("send_message", { room, session, to, text, then: then ?? null }),
  ping: (id: string) => call<string>("ping_provider", { id }),
  createRoom: (path: string, members: string[] = []) => call<string>("create_room", { path, members }),
  addBot: (name: string) => call<string>("add_bot", { name }),
  removeBot: (id: string) => call<void>("remove_bot", { id }),
  setBotCanAsk: (id: string, on: boolean) => call<void>("set_bot_can_ask", { id, on }),
  renameAgent: (id: string, name: string, description?: string) => call<void>("rename_agent", { id, name, description: description ?? null }),
  newSession: (room: string, title: string) => call<string>("new_session", { room, title }),
  setContext: (room: string, kind: "notes" | "terminal" | "diff", content: string) =>
    call<void>("set_context", { room, kind, content }),
  refreshGit: (room: string) => call<void>("refresh_git", { room }),
  setAllowEdits: (room: string, allow: boolean) => call<void>("set_allow_edits", { room, allow }),
  search: (query: string) =>
    call<{ roomId: string; sender: string; text: string; createdAt: string }[]>("search", { query }),
  vaultFiles: () => call<VaultFile[]>("vault_files"),
  saveVaultFile: (filename: string, content: string) => call<void>("save_vault_file", { filename, content }),
  mcp: (request: unknown, cwd?: string) => call<any>("mcp_rpc", { request, cwd: cwd ?? null }),
  grokStatus: (agent: string) => call<GrokWebhook>("grok_webhook_status", { agent }),
  grokSave: (agent: string, url: string, key: string) => call<GrokWebhook>("grok_webhook_save", { agent, url, key }),
  grokClear: (agent: string) => call<GrokWebhook>("grok_webhook_clear", { agent }),
  grokTest: (agent: string) => call<string>("grok_webhook_test", { agent }),
  appVersion: () => call<{ version: string; autoCheck: boolean }>("app_version"),
  setAutoUpdate: (on: boolean) => call<void>("set_auto_update", { on }),
  checkUpdate: () => call<{ available: boolean; version?: string; notes?: string | null }>("check_update"),
  installUpdate: () => call<void>("install_update"),
  gatewayStatus: () => call<Gateway>("gateway_status"),
  gatewayStart: () => call<Gateway>("gateway_start"),
  gatewayStop: () => call<Gateway>("gateway_stop"),
  gatewaySetProvider: (provider: TunnelProvider, ngrokDomain?: string) =>
    call<Gateway>("gateway_set_provider", { provider, ngrokDomain: ngrokDomain ?? null }),
  cfLogin: () => call<Gateway>("tunnel_cf_login"),
  cfCreate: (hostname: string) => call<Gateway>("tunnel_cf_create", { hostname }),
  cfToken: (token: string, hostname: string) => call<Gateway>("tunnel_cf_token", { token, hostname }),
  cfSwitchDomain: () => call<Gateway>("tunnel_cf_switch_domain"),
  tunnelVerify: () => call<string>("tunnel_verify"),
  cfReset: () => call<Gateway>("tunnel_cf_reset"),
  ngrokSetup: (authtoken: string | null, domain: string | null) => call<Gateway>("tunnel_ngrok_setup", { authtoken, domain }),
  setAutostart: (on: boolean) => call<Gateway>("tunnel_set_autostart", { on }),
  gatewayReveal: (agent: string) => call<{ token: string; mcpUrl: string | null }>("gateway_reveal", { agent }),
  gatewayRotate: (agent: string) => call<{ token: string; mcpUrl: string | null }>("gateway_rotate_token", { agent }),
};

export const onMuxEvent = (cb: (e: MuxEvent) => void) => listen<MuxEvent>("mux://event", (ev) => cb(ev.payload));

export function ago(iso: string | null | undefined): string {
  if (!iso) return "never";
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 10) return "just now";
  if (s < 60) return `${Math.floor(s)}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function clock(iso: string): string {
  return new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** Native folder picker; null if cancelled. */
export async function pickFolder(): Promise<string | null> {
  const r = await openDialog({ directory: true, multiple: false, title: "Choose a workspace folder" });
  return typeof r === "string" ? r : null;
}
