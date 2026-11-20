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
}

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
export type PlatformKind = 'qq_official' | 'weixin_ilink'

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

export type PluginStatus = 'installing' | 'running' | 'stopped' | 'error' | 'over_quota'

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

export type PluginInspect = {
  name: string
  display_name: string
  version: string
  author: string
  description: string
  repo_url: string
  schema: PluginSchema | null
  has_requirements: boolean
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
  storage_bytes: number
  cpu_percent: number
}

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
  redemptions: { user_id: string; user_email: string; days: number; redeemed_at: number }[]
}

export type AdminOverview = {
  users: { total: number; new_7d: number; active: number; expiring_7d: number; expired: number }
  invites: { unused: number; used_7d: number }
  bots: { total: number; running: number; starting: number; stopped: number; error: number }
  plugins: { total: number; running: number; installing: number; error: number; over_quota: number }
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
