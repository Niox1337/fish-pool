import type {
  QqMode,
  QqPanel,
  QqPanelRecord,
  QqPanelScope,
  AdminOverview,
  AdminSystem,
  AdminUser,
  Invite,
  Bot,
  BotSettings,
  CreateBotInput,
  KeyBalance,
  GatewayAccount,
  AdminCredits,
  LlmKey,
  LogLine,
  Platform,
  Plugin,
  PluginInspect,
  QrStatus,
  QrTask,
  Quota,
  Usage,
  User,
} from './types'

export class ApiError extends Error {
  code: string
  status: number
  constructor(status: number, code: string, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' }
  if (method !== 'GET') headers['X-Fish-Csrf'] = '1'
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  let res: Response
  try {
    if (import.meta.env.VITE_MOCK === '1') {
      const { mockFetch } = await import('./mock')
      res = await mockFetch(method, path, body)
    } else {
      res = await fetch(`/api${path}`, {
        method,
        headers,
        credentials: 'same-origin',
        body: body === undefined ? undefined : JSON.stringify(body),
      })
    }
  } catch {
    throw new ApiError(0, 'network', '网络好像断了，稍后再试试')
  }
  const text = await res.text()
  const data = text ? safeJson(text) : null
  if (!res.ok) {
    const err = (data as { error?: { code?: string; message?: string } } | null)?.error
    throw new ApiError(res.status, err?.code ?? 'http_' + res.status, err?.message ?? `请求失败（${res.status}）`)
  }
  return data as T
}

function safeJson(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

const get = <T>(p: string) => request<T>('GET', p)
const post = <T>(p: string, b?: unknown) => request<T>('POST', p, b ?? {})
const patch = <T>(p: string, b: unknown) => request<T>('PATCH', p, b)
const del = <T>(p: string) => request<T>('DELETE', p)
const qqPanelsPath = (id: string, pid: string) => `/bots/${id}/platforms/${pid}/qq-panels`

export const api = {
  captcha: () => get<{ id: string; svg: string }>('/captcha'),
  sendCode: (b: { email: string; purpose: 'register' | 'reset'; captcha_id: string; captcha_answer: string; invite_code?: string }) =>
    post<{ ok: true; cooldown_s: number }>('/auth/email-code', b),
  register: (b: { email: string; password: string; code: string; invite_code: string; agree: true }) => post<{ user: User }>('/auth/register', b),
  login: (b: { email: string; password: string; captcha_id?: string; captcha_answer?: string }) =>
    post<{ user: User }>('/auth/login', b),
  logout: () => post<{ ok: true }>('/auth/logout'),
  reset: (b: { email: string; code: string; password: string }) => post<{ ok: true }>('/auth/reset', b),
  me: () => get<{ user: User }>('/me'),
  redeem: (code: string) => post<{ user: User }>('/me/redeem', { code }),

  llmKeys: () => get<{ items: LlmKey[] }>('/llm-keys'),
  addLlmKey: (b: { name: string; base_url: string; model: string; api_key: string }) => post<LlmKey>('/llm-keys', b),
  deleteLlmKey: (id: string) => del<{ ok: true }>(`/llm-keys/${id}`),
  llmKeyBalance: (id: string) => get<KeyBalance>(`/llm-keys/${id}/balance`),
  gateway: () => get<GatewayAccount>('/me/gateway'),
  rotateGateway: () => post<{ key: LlmKey }>('/me/gateway/rotate'),

  bots: () => get<{ items: Bot[] }>('/bots'),
  bot: (id: string) => get<Bot>(`/bots/${id}`),
  createBot: (input: CreateBotInput) => post<Bot>('/bots', input),
  updateBot: (id: string, b: { name?: string; avatar?: string; settings?: BotSettings }) => patch<Bot>(`/bots/${id}`, b),
  botAction: (id: string, action: 'start' | 'stop' | 'restart') => post<Bot>(`/bots/${id}/${action}`),
  deleteBot: (id: string) => del<{ ok: true }>(`/bots/${id}`),
  logs: (id: string, after?: string) =>
    get<{ items: LogLine[]; cursor: string }>(`/bots/${id}/logs${after ? `?after=${encodeURIComponent(after)}` : ''}`),

  qrStart: (id: string, kind: 'qq' | 'weixin') => post<QrTask>(`/bots/${id}/platforms/${kind}/qr`),
  qrPoll: (id: string, kind: 'qq' | 'weixin', task: string) => get<QrStatus>(`/bots/${id}/platforms/${kind}/qr/${task}`),
  qqManual: (id: string, b: { app_id: string; secret: string }) => post<Platform>(`/bots/${id}/platforms/qq/manual`, b),
  updatePlatform: (id: string, pid: string, b: { enabled?: boolean; display_name?: string; mode?: QqMode }) =>
    patch<Platform>(`/bots/${id}/platforms/${pid}`, b),
  deletePlatform: (id: string, pid: string) => del<{ ok: true }>(`/bots/${id}/platforms/${pid}`),
  qqPanels: (id: string, pid: string, scope: QqPanelScope) => get<{ records: QqPanelRecord[] }>(`${qqPanelsPath(id, pid)}?scope=${scope}`),
  createQqPanel: (id: string, pid: string, scope: QqPanelScope, panel: QqPanel) =>
    post<{ panel_id: string }>(qqPanelsPath(id, pid), { scope, panel }),
  updateQqPanel: (id: string, pid: string, panelId: string, panel: QqPanel, expectedVersion?: number) =>
    request<{ version: number }>('PUT', `${qqPanelsPath(id, pid)}/${encodeURIComponent(panelId)}`, { panel, expected_version: expectedVersion }),
  deleteQqPanel: (id: string, pid: string, panelId: string) =>
    del<{ ok: true }>(`${qqPanelsPath(id, pid)}/${encodeURIComponent(panelId)}`),

  inspectPlugin: (repo_url: string) => post<PluginInspect>('/plugins/inspect', { repo_url }),
  addPlugin: (id: string, repo_url: string) => post<Plugin>(`/bots/${id}/plugins`, { repo_url }),
  updatePlugin: (id: string, pid: string, b: { enabled?: boolean; config?: Record<string, unknown>; config_revision?: string }) =>
    patch<Plugin>(`/bots/${id}/plugins/${pid}`, b),
  upgradePlugin: (id: string, pid: string) => post<Plugin>(`/bots/${id}/plugins/${pid}/update`),
  reloadPlugin: (id: string, pid: string) => post<{ ok: true }>(`/bots/${id}/plugins/${pid}/reload`),
  deletePlugin: (id: string, pid: string) => del<{ ok: true }>(`/bots/${id}/plugins/${pid}`),

  usage: () => get<Usage>('/usage'),

  adminOverview: () => get<AdminOverview>('/admin/overview'),
  adminSystem: () => get<AdminSystem>('/admin/system'),
  adminUsers: (q?: string, cursor?: string) => {
    const p = new URLSearchParams()
    if (q) p.set('q', q)
    if (cursor) p.set('cursor', cursor)
    const qs = p.toString()
    return get<{ items: AdminUser[]; cursor: string | null }>(`/admin/users${qs ? `?${qs}` : ''}`)
  },
  adminUser: (id: string) => get<{ user: AdminUser; bots: Bot[] }>(`/admin/users/${id}`),
  adminUserUsage: (id: string) => get<Usage>(`/admin/users/${id}/usage`),
  adminBotLogs: (uid: string, bid: string) => get<{ items: LogLine[]; cursor: string }>(`/admin/users/${uid}/bots/${bid}/logs`),
  adminPatchUser: (id: string, b: { quota?: Partial<User['quota']>; service_until?: number | null; extend_days?: number }) =>
    patch<AdminUser>(`/admin/users/${id}`, b),
  adminInvites: (status: 'unused' | 'used' | 'revoked' | 'all') =>
    get<{ items: Invite[]; cursor: string | null; default_quota: Quota }>(`/admin/invites?status=${status}`),
  adminCredits: (userId: string) => get<AdminCredits>(`/admin/users/${userId}/credits`),
  adminAddCredit: (userId: string, b: { amount: string; expires_at: number | null; note?: string }) =>
    post<AdminCredits>(`/admin/users/${userId}/credits`, b),
  adminPatchCredit: (userId: string, creditId: string, b: { amount?: string; expires_at?: number | null }) =>
    patch<AdminCredits>(`/admin/users/${userId}/credits/${creditId}`, b),
  adminCreateInvites: (b: { days: number; count: number; quota: Quota; max_uses?: number; note?: string; expires_at?: number | null; gateway_budget?: string }) =>
    post<{ items: Invite[] }>('/admin/invites', b),
  adminRevokeInvite: (code: string) => del<{ ok: true }>(`/admin/invites/${encodeURIComponent(code)}`),
}
