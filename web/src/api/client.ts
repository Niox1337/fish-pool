import type {
  AdminOverview,
  AdminUser,
  Invite,
  Bot,
  BotSettings,
  LlmKey,
  LogLine,
  Platform,
  Plugin,
  PluginInspect,
  QrStatus,
  QrTask,
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

  bots: () => get<{ items: Bot[] }>('/bots'),
  bot: (id: string) => get<Bot>(`/bots/${id}`),
  createBot: (name: string) => post<Bot>('/bots', { name }),
  updateBot: (id: string, b: { name?: string; avatar?: string; settings?: BotSettings }) => patch<Bot>(`/bots/${id}`, b),
  botAction: (id: string, action: 'start' | 'stop' | 'restart') => post<Bot>(`/bots/${id}/${action}`),
  deleteBot: (id: string) => del<{ ok: true }>(`/bots/${id}`),
  logs: (id: string, after?: string) =>
    get<{ items: LogLine[]; cursor: string }>(`/bots/${id}/logs${after ? `?after=${encodeURIComponent(after)}` : ''}`),

  qrStart: (id: string, kind: 'qq' | 'weixin') => post<QrTask>(`/bots/${id}/platforms/${kind}/qr`),
  qrPoll: (id: string, kind: 'qq' | 'weixin', task: string) => get<QrStatus>(`/bots/${id}/platforms/${kind}/qr/${task}`),
  qqManual: (id: string, b: { app_id: string; secret: string }) => post<Platform>(`/bots/${id}/platforms/qq/manual`, b),
  updatePlatform: (id: string, pid: string, b: { enabled?: boolean; display_name?: string }) =>
    patch<Platform>(`/bots/${id}/platforms/${pid}`, b),
  deletePlatform: (id: string, pid: string) => del<{ ok: true }>(`/bots/${id}/platforms/${pid}`),

  inspectPlugin: (repo_url: string) => post<PluginInspect>('/plugins/inspect', { repo_url }),
  addPlugin: (id: string, repo_url: string) => post<Plugin>(`/bots/${id}/plugins`, { repo_url }),
  updatePlugin: (id: string, pid: string, b: { enabled?: boolean; config?: Record<string, unknown> }) =>
    patch<Plugin>(`/bots/${id}/plugins/${pid}`, b),
  upgradePlugin: (id: string, pid: string) => post<Plugin>(`/bots/${id}/plugins/${pid}/update`),
  deletePlugin: (id: string, pid: string) => del<{ ok: true }>(`/bots/${id}/plugins/${pid}`),

  usage: () => get<Usage>('/usage'),

  adminOverview: () => get<AdminOverview>('/admin/overview'),
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
    get<{ items: Invite[]; cursor: string | null }>(`/admin/invites?status=${status}`),
  adminCreateInvites: (b: { days: number; count: number; max_uses?: number; note?: string; expires_at?: number | null }) =>
    post<{ items: Invite[] }>('/admin/invites', b),
  adminRevokeInvite: (code: string) => del<{ ok: true }>(`/admin/invites/${encodeURIComponent(code)}`),
}
