import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

/** `cursor`, `claude`, `chatgpt`, `grok`, or an added bot such as `grok:job-hunter`. */
export type AgentId = string;
/** A provider type as the backend registry describes it (`providers::catalog`). */
export interface CatalogEntry {
  id: string;
  name: string;
  color: string;
  hint: string;
  multiInstance: boolean;
  remoteAccess: boolean;
  wake: "cli" | "webhook" | "inbox";
  contextHook: boolean;
}

/** Shown before the first overview arrives; the backend catalog replaces it. */
const FALLBACK: CatalogEntry[] = [
  { id: "cursor", name: "Cursor", color: "#3B82F6", hint: "MCP stdio · ~/.cursor/mcp.json", multiInstance: false, remoteAccess: false, wake: "cli", contextHook: false },
  { id: "grok", name: "Grok Bot", color: "#D97736", hint: "MCP stdio · added in Grok Bot settings", multiInstance: true, remoteAccess: true, wake: "webhook", contextHook: false },
  { id: "claude", name: "Claude Code", color: "#10B981", hint: "MCP stdio · claude mcp add", multiInstance: false, remoteAccess: false, wake: "cli", contextHook: true },
  { id: "chatgpt", name: "ChatGPT", color: "#8B5CF6", hint: "MCP stdio · ~/.codex/config.toml", multiInstance: false, remoteAccess: false, wake: "cli", contextHook: false },
];

let known: Provider[] = [];
let catalog: CatalogEntry[] = FALLBACK;
/** Called whenever the overview refreshes so names/colours of added bots and custom providers resolve everywhere. */
export const setKnownProviders = (ps: Provider[], cat?: CatalogEntry[]) => {
  known = ps;
  if (cat?.length) catalog = cat;
};
export const getCatalog = () => catalog;

/** The provider type an agent id belongs to: `grok:job-hunter` → grok. */
export function providerOf(id: string): CatalogEntry | undefined {
  const base = id.split(":")[0];
  return catalog.find((c) => c.id === id) ?? catalog.find((c) => c.id === base && c.multiInstance);
}

export const isGrok = (id: string) => providerOf(id)?.id === "grok";

/** Display info for any agent id, including bots and their self-registered names. */
export function agentMeta(id: string): { name: string; handle: string; color: string; subtitle: string | null } {
  const p = known.find((x) => x.id === id);
  const type = providerOf(id);
  const name = p?.name ?? type?.name ?? id;
  return {
    name,
    handle: `@${id}`,
    color: type?.color ?? "#6B7280",
    subtitle: type?.multiInstance && type.id !== id && name !== type.name ? type.name : null,
  };
}

export interface SyncStatus {
  configured: boolean;
  remote: string | null;
  includeDiffs: boolean;
  auto: boolean;
  last: string | null;
  error: string | null;
  visibility: "private" | "unverified" | null;
  gitInstalled: boolean;
  dir: string;
}
export interface SyncReport {
  imported: { rooms: number; sessions: number; messages: number; bots: number };
  pushed: boolean;
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
  catalog: CatalogEntry[];
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
/** The process listening on the HTTP gateway port when this app couldn't bind it. */
export interface PortHolder {
  pid: number;
  path: string;
  agentmux: boolean;
}
export interface Detect {
  agent: AgentId;
  installed: boolean;
  configured: boolean;
  detail: string;
  contextHook: boolean;
  options: Record<string, boolean>;
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
  setProviderOption: (id: string, name: string, on: boolean) => call<string>("set_provider_option", { id, name, on }),
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
  syncStatus: () => call<SyncStatus>("sync_status"),
  syncConnect: (remote: string, includeDiffs: boolean, auto: boolean) => call<{ status: SyncStatus; report: SyncReport }>("sync_connect", { remote, includeDiffs, auto }),
  syncNow: () => call<{ status: SyncStatus; report: SyncReport }>("sync_now"),
  syncSetOptions: (includeDiffs: boolean, auto: boolean) => call<SyncStatus>("sync_set_options", { includeDiffs, auto }),
  syncDisconnect: () => call<SyncStatus>("sync_disconnect"),
  setDaemon: (on: boolean) => call<string>("set_daemon", { on }),
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
  portHolder: () => call<PortHolder | null>("gateway_port_holder"),
  takeOverPort: () => call<string>("gateway_take_over_port"),
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
