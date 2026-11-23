export type Quota = { memory_mb: number; storage_mb: number; bots: number | null; plugins: number | null }

export type User = {
  id: string
  email: string
  role: 'user' | 'admin'
  created_at: number
  quota: Quota
  service_until: number | null
  service_active: boolean
}

export type LlmKey = {
  id: string
  name: string
  base_url: string
  model: string
  key_masked: string
  created_at: number
  /** The node reaches this provider through the Cloudflare AI Gateway. */
  via_gateway?: boolean
  /** A gengwiki gateway key, its balance comes from `/llm-keys/{id}/balance`. */
  has_balance?: boolean
  /** The user's event credit key, its secret can be replaced with `/me/gateway/rotate`. */
  rotatable?: boolean
}

/** One usable event credit, in billing order. */
type BalanceCredit = {
  available: string
  amount: string
  spent: string
  expires_at: string | null
}

export type KeyBalance = {
  currency: string
  available: string
  budget: string
  spent: string
  expires_at: string | null
  disabled: boolean
  credits?: BalanceCredit[]
}

/** The user's event credit key with its balance and credits not yet sent to the gateway. */
export type GatewayAccount = {
  account: boolean
  key: LlmKey | null
  balance: KeyBalance | null
  balance_error?: string | null
  pending: { amount: string; expires_at: number | null }[]
}

export type AdminCredit = {
  id: string
  source: 'invite' | 'admin'
  amount: string
  expires_at: number | null
  expired: boolean
  note: string
  created_at: number
  created_by_email: string | null
  state: 'pending' | 'failed' | 'granted'
  error: string | null
  spent: string | null
  remaining: string | null
}

export type AdminCredits = {
  configured: boolean
  account: boolean
  items: AdminCredit[]
  remote_error: string | null
}

export type CreateBotInput = { name: string; key_id: string; model: string }

export type BotSettings = {
  persona: { name: string; system_prompt: string; greeting: string }
  llm: {
    key_id: string | null
    model: string
    temperature: number
    max_tokens: number
    context_turns: number
  }
  behavior: {
    wake_prefixes: string[]
    reply_when_at: boolean
    reply_in_private: boolean
    reply_in_group_without_at: boolean
    admins: string[]
    blocklist: string[]
    rate_limit_per_minute: number
  }
}

export type BotStatus = 'starting' | 'running' | 'stopped' | 'error'
type PlatformKind = 'qq_official' | 'weixin_ilink'

export type Platform = {
  id: string
  kind: PlatformKind
  display_name: string
  app_id: string
  secret_masked: string
  enabled: boolean
  status: 'connected' | 'connecting' | 'error' | 'disabled'
  status_detail?: string
  /** QQ only: how events reach the bot. */
  mode?: QqMode
}

export type QqMode = 'websocket' | 'webhook'

export type QqPanelScope = 'c2c' | 'group'

export type QqPanelItem = {
  type: 'command' | 'link'
  name: string
  desc?: string
  only_admin?: boolean
  link?: string
}

export type QqPanel = { items: QqPanelItem[]; remark?: string; version?: number }
export type QqPanelRecord = {
  panel_id: string
  scope: QqPanelScope
  target_type: 'all' | 'specific'
  panel: QqPanel
  version?: number
}

export type SchemaField = {
  type: 'string' | 'text' | 'int' | 'float' | 'bool' | 'list' | 'object' | 'dict' | 'template_list' | 'file'
  description?: string
  hint?: string
  obvious_hint?: boolean
  default?: unknown
  options?: (string | number)[]
  labels?: string[]
  secret?: boolean
  slider?: { min: number; max: number; step?: number }
  invisible?: boolean
  condition?: Record<string, unknown>
  editor_mode?: boolean
  items?: Record<string, SchemaField>
  templates?: Record<string, { name?: string; hint?: string; display_item?: string; hide_hint_in_list?: boolean; items?: PluginSchema }>
}

export type PluginSchema = Record<string, SchemaField>

export type PluginStatus = 'installing' | 'running' | 'sleeping' | 'stopped' | 'error' | 'over_quota'

export type Plugin = {
  id: string
  repo_url: string
  name: string
  display_name: string
  version: string
  author: string
  description: string
  enabled: boolean
  config: Record<string, unknown>
  config_revision?: string
  schema: PluginSchema | null
  status: PluginStatus
  status_detail?: string
  /** Commit the plugin is pinned to. Empty or missing installs the head of its branch. */
  commit?: string
  updated_at: number
}

export type Bot = {
  id: string
  name: string
  avatar: string | null
  status: BotStatus
  status_detail?: string
  created_at: number
  updated_at: number
  settings: BotSettings
  platforms: Platform[]
  plugins: Plugin[]
}

export type MarketPlugin = {
  id: string
  name: string
  display_name: string
  author: string
  description: string
  repo_url: string
  category: string
  tags: string[]
}

export type QrTask = { task_id: string; qr_url: string; interval_s: number; expires_at: number }
export type QrStatus = {
  status: 'pending' | 'scanned' | 'expired' | 'created' | 'error'
  message?: string
  platform?: Platform
}

export type UsageSource = {
  bot_id: string
  kind: 'core' | 'conversations' | 'plugin'
  plugin_id: string | null
  label: string
  memory_bytes: number
  /** Files the account owns, counted against the storage quota. */
  storage_bytes: number
  /** Shared code and dependency install of a plugin. Not counted against the quota. */
  deps_bytes?: number
  cpu_percent: number
}

/** One stored chat session, keyed by `platform_id:MessageType:session_id`. */
export type ConversationSession = { session: string; messages: number; bytes: number; last_ts: number }

export type Usage = {
  quota: { memory_mb: number; storage_mb: number }
  memory_bytes: number
  storage_bytes: number
  sources: UsageSource[]
  history: { ts: number; memory_bytes: number; storage_bytes: number }[]
}

export type LogLine = { ts: number; level: 'debug' | 'info' | 'warn' | 'error'; source: string; message: string }

export type AdminUser = User & {
  bots: number
  plugins: number
  running: number
  errors: number
  memory_bytes: number | null
  storage_bytes: number | null
  last_login_at: number | null
}

export type AdminNode = {
  id: string
  base_url: string
  online: boolean
  version: string
  memory_total_bytes: number
  memory_used_bytes: number
  bots: number
  last_seen: number
  error?: string
}

export type AdminProblem = {
  user_id: string
  user_email: string
  bot_id: string
  bot_name: string
  kind: 'bot' | 'platform' | 'plugin'
  id: string
  label: string
  status: string
  status_detail?: string
  updated_at: number
}

export type Invite = {
  code: string
  days: number
  quota: Quota | null
  max_uses: number
  used_count: number
  note: string | null
  created_by_email: string
  created_at: number
  expires_at: number | null
  revoked_at: number | null
  /** gengwiki gateway credit in yuan each redemption gets as a model key. */
  gateway_budget?: string | null
  redemptions: {
    user_id: string
    user_email: string
    days: number
    redeemed_at: number
    gateway?: 'pending' | 'failed' | 'granted' | null
    gateway_error?: string | null
  }[]
}

export type AdminOverview = {
  users: { total: number; new_7d: number; active: number; expiring_7d: number; expired: number }
  invites: { unused: number; used_7d: number }
  bots: { total: number; running: number; starting: number; stopped: number; error: number }
  plugins: { total: number; running: number; sleeping: number; installing: number; error: number; over_quota: number }
  platforms: { qq_official: number; weixin_ilink: number; error: number }
  usage: { memory_bytes: number; storage_bytes: number; quota_memory_mb: number; quota_storage_mb: number }
  plugin_cpu: {
    percent: number | null
    reporting_nodes: number
    items: {
      node_id: string
      sampled_at: number
      user_id: string
      bot_id: string
      bot_name: string
      plugin_id: string
      label: string
      cpu_percent: number
    }[]
  }
  nodes: AdminNode[]
  problems: AdminProblem[]
}

type SysPluginRef = { user_id: string; bot_id: string; bot_name: string; plugin_id: string; label: string }

export type SysProcess = {
  pid: number
  ppid: number
  role: 'node' | 'sidecar' | 'plugin' | 'task' | 'other'
  name: string
  rss_bytes: number
  pss_bytes: number | null
  uss_bytes: number | null
  swap_bytes: number | null
  cpu_percent: number | null
  threads: number | null
  started_at: number | null
  plugin?: SysPluginRef
}

export type SysPoint = {
  ts: number
  host_cpu: number | null
  container_cpu: number | null
  host_memory_used_bytes: number | null
  container_memory_bytes: number | null
}

export type SysCounts = { total?: number } & Record<string, number | string | undefined>

export type NodeSystem = {
  node_id: string
  version: string
  started_at: number
  interval_s: number
  sampled_at: number | null
  host: {
    cpu_count: number
    cpu_percent: number | null
    load: [number, number, number] | null
    uptime_s: number | null
    memory_total_bytes: number
    memory_available_bytes: number | null
    swap_total_bytes: number | null
    swap_free_bytes: number | null
    kernel: string | null
  } | null
  container: {
    memory_current_bytes: number
    memory_max_bytes: number | null
    memory_peak_bytes: number | null
    memory_anon_bytes: number | null
    memory_file_bytes: number | null
    oom_kills: number
    cpu_percent: number | null
    cpu_limit_cores: number | null
    pids_current: number | null
    pids_max: number | null
  } | null
  processes: SysProcess[]
  history: SysPoint[]
  disk: { path: string; total_bytes: number; free_bytes: number; avail_bytes: number; inodes_total: number; inodes_free: number } | null
  storage: { node_db_bytes: number; conversations_db_bytes: number; tenants_bytes: number; uv_cache_bytes: number | null }
  cgroups_enabled: boolean
  tenants: {
    user_id: string
    memory_current_bytes: number
    memory_max_bytes: number | null
    memory_peak_bytes: number | null
    oom_kills: number
    procs: number
  }[]
  bots: SysCounts
  platforms: ({ kind: string } & SysCounts)[]
  plugins: SysCounts
  logs: {
    window_s: number
    errors: number
    warns: number
    bots: {
      bot_id: string
      bot_name: string
      user_id: string
      errors: number
      warns: number
      last_error_at: number | null
      last_error: string | null
    }[]
  }
}

export type AdminSystemNode = {
  id: string
  base_url: string
  online: boolean
  latency_ms: number
  last_seen: number | null
  error?: string
  system: NodeSystem | null
}

export type AdminSystem = {
  generated_at: number
  console: {
    colo: string | null
    d1: {
      size_bytes: number | null
      query_ms: number | null
      round_trip_ms: number
      tables: { name: string; rows: number }[]
      users_admin: number
      users_login_24h: number
      sessions_active: number
      bots_desired_running: number
      bots_needs_push: number
      qr_tasks_active: number
    }
  }
  nodes: AdminSystemNode[]
  users: Record<string, string>
}
