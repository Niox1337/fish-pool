import type { AdminOverview, AdminUser, Bot, Invite, LlmKey, LogLine, Plugin, Usage, User } from './types'

const MB = 1024 * 1024
const now = () => Date.now()
const id = () => Math.random().toString(36).slice(2, 12)

const defaultSettings = (): Bot['settings'] => ({
  persona: { name: '大肥鱼', system_prompt: '你是一条爱吃白饭的蓝色大肥鱼，说话软乎乎的，喜欢帮群友解答问题。', greeting: '' },
  llm: { key_id: null, model: 'deepseek-chat', temperature: 0.7, max_tokens: 1024, context_turns: 12 },
  behavior: {
    wake_prefixes: ['/'],
    reply_when_at: true,
    reply_in_private: true,
    reply_in_group_without_at: false,
    admins: [],
    blocklist: [],
    rate_limit_per_minute: 20,
  },
})

type State = { user: User | null; keys: LlmKey[]; bots: Bot[]; qr: Record<string, number> }

const state: State = {
  user: null,
  keys: [],
  bots: [],
  qr: {},
}

const botLogs: Record<string, LogLine[]> = {}
const logPool: Omit<LogLine, 'ts'>[] = [
  { level: 'info', source: 'qq_official', message: '收到群消息 [群 7342] 小明：/一言' },
  { level: 'info', source: 'llm', message: '调用模型完成，用时 1.4 秒，消耗 312 token' },
  { level: 'debug', source: 'core', message: '心跳正常，事件队列 0' },
  { level: 'info', source: 'weixin_ilink', message: '收到私聊消息，24 小时回复窗口已刷新' },
  { level: 'warn', source: 'llm', message: '模型回复较慢，用时 6.2 秒' },
  { level: 'info', source: 'core', message: '回复已发送' },
  { level: 'error', source: 'llm', message: '模型接口返回 401：API Key 无效或已过期' },
]
function pushLog(botId: string, line: Omit<LogLine, 'ts'>) {
  const list = (botLogs[botId] ??= [])
  list.push({ ...line, ts: now() })
  if (list.length > 400) list.splice(0, list.length - 400)
}
function setRunning(b: Bot, delay: number) {
  b.status = 'starting'
  b.status_detail = undefined
  setTimeout(() => {
    if (!state.bots.includes(b) || b.status !== 'starting') return
    b.status = 'running'
    pushLog(b.id, { level: 'info', source: 'core', message: '小鱼醒了，开始上课' })
    for (const p of b.platforms) pushLog(b.id, { level: 'info', source: p.kind, message: `已连上${p.display_name}` })
    for (const p of b.plugins) pushLog(b.id, { level: 'info', source: `plugin:${p.name}`, message: '插件已加载' })
  }, delay)
}

export const mockDev = {
  state,
  botLogs,
  qrMode: 'normal' as 'normal' | 'expire' | 'error',
}

const samplePlugin = (name: string, display: string, mem: number): Plugin => ({
  id: id(),
  repo_url: `https://github.com/example/${name}`,
  name,
  display_name: display,
  version: 'v1.2.0',
  author: 'example',
  description: `${display}的示例插件`,
  enabled: true,
  config: { daily_limit: 5 },
  schema: {
    daily_limit: { type: 'int', description: '每人每天最多触发次数', default: 5 },
    nickname: { type: 'string', description: '插件里怎么称呼群友', hint: '留空就用群名片', default: '' },
    enable_image: { type: 'bool', description: '回复时附带图片', default: true },
    image_size: { type: 'string', description: '图片尺寸', options: ['small', 'large'], labels: ['小图', '大图'], default: 'small', condition: { enable_image: true } },
    style: { type: 'string', description: '语气', options: ['可爱', '正经', '毒舌'], default: '可爱' },
    chance: { type: 'float', description: '随机接话概率', slider: { min: 0, max: 1, step: 0.05 }, default: 0.1 },
    intro: { type: 'text', description: '自我介绍', hint: '有人发 /介绍 时回复这段话', obvious_hint: true, default: '' },
    keywords: { type: 'list', description: '触发关键词', default: ['早安', '晚安'] },
    api: {
      type: 'object',
      description: '图源接口',
      items: {
        endpoint: { type: 'string', description: '接口地址', default: 'https://api.example.com' },
        timeout: { type: 'int', description: '超时秒数', default: 10 },
      },
    },
    templates: { type: 'template_list', description: '回复模板', default: [{ name: '默认', text: '今天也要好好吃饭' }] },
    secret_token: { type: 'string', description: '内部令牌', invisible: true, default: 'x' },
  },
  status: 'running',
  updated_at: now(),
  ...{ _mem: mem },
})

function seedUser(email: string): User {
  return {
    id: id(),
    email,
    role: email.startsWith('admin') ? 'admin' : 'user',
    created_at: now(),
    quota: { memory_mb: 256, storage_mb: 1024, bots: 1, plugins: 8 },
    service_until: now() + 30 * 86400000,
    service_active: true,
  }
}

const normCode = (c: string) => c.toUpperCase().replace(/[\s-]/g, '').replace(/O/g, '0').replace(/[IL]/g, '1')
const fmtCode = (raw: string) => `FISH-${raw.slice(4, 8)}-${raw.slice(8, 12)}`
const genCode = () => {
  const al = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
  let r = 'FISH'
  for (let i = 0; i < 8; i++) r += al[Math.floor(Math.random() * 32)]
  return fmtCode(r)
}
const invites: Invite[] = [
  { code: 'FISH-7K2M-Q9TD', days: 30, max_uses: 1, used_count: 0, note: '群友试用', created_by_email: 'admin@fish.niox.lol', created_at: now() - 86400000, expires_at: null, revoked_at: null, redemptions: [] },
  { code: 'FISH-9WQE-2RJA', days: 7, max_uses: 5, used_count: 2, note: '活动群批量', created_by_email: 'admin@fish.niox.lol', created_at: now() - 9 * 86400000, expires_at: now() - 2 * 86400000, revoked_at: now() - 3 * 86400000, redemptions: [{ user_id: 'u0', user_email: 'xiaoyu@example.com', days: 7, redeemed_at: now() - 8 * 86400000 }, { user_id: 'u1', user_email: 'qunzhu@example.com', days: 7, redeemed_at: now() - 6 * 86400000 }] },
  { code: 'FISH-3HXV-PB6N', days: 90, max_uses: 1, used_count: 1, note: null, created_by_email: 'admin@fish.niox.lol', created_at: now() - 5 * 86400000, expires_at: null, revoked_at: null, redemptions: [{ user_id: 'u2', user_email: 'astr.fan@example.com', days: 90, redeemed_at: now() - 4 * 86400000 }] },
]
function takeInvite(code: string): Invite | Response {
  const inv = invites.find((i) => normCode(i.code) === normCode(code ?? ''))
  if (!inv || inv.revoked_at) return err(400, 'invite_invalid', '邀请码不对，再核对一下')
  if (inv.used_count >= inv.max_uses) return err(400, 'invite_used', '这个邀请码已经用过了')
  if (inv.expires_at && inv.expires_at < now()) return err(400, 'invite_expired', '这个邀请码过期了')
  return inv
}

function err(status: number, code: string, message: string) {
  return new Response(JSON.stringify({ error: { code, message } }), { status })
}
const ok = (data: unknown) => new Response(JSON.stringify(data), { status: 200 })

function usage(): Usage {
  const sources: Usage['sources'] = []
  for (const b of state.bots) {
    sources.push({ bot_id: b.id, kind: 'core', plugin_id: null, label: '核心连接', memory_bytes: 9.6 * MB, storage_bytes: 0.2 * MB, cpu_percent: 0.4 })
    sources.push({ bot_id: b.id, kind: 'conversations', plugin_id: null, label: '聊天记录', memory_bytes: 0, storage_bytes: 3.1 * MB, cpu_percent: 0 })
    for (const p of b.plugins) {
      const mem = ((p as unknown as { _mem?: number })._mem ?? 24) * MB
      sources.push({ bot_id: b.id, kind: 'plugin', plugin_id: p.id, label: p.display_name, memory_bytes: p.enabled ? mem : 0, storage_bytes: mem * 2.4, cpu_percent: p.enabled ? 1.2 : 0 })
    }
  }
  const memory_bytes = sources.reduce((s, x) => s + x.memory_bytes, 0)
  const storage_bytes = sources.reduce((s, x) => s + x.storage_bytes, 0)
  const history = Array.from({ length: 288 }, (_, i) => {
    const t = now() - (287 - i) * 5 * 60 * 1000
    const wave = Math.sin(i / 18) * 0.12 + Math.sin(i / 5) * 0.04
    return { ts: t, memory_bytes: Math.max(0, memory_bytes * (0.82 + wave)), storage_bytes: storage_bytes * (0.9 + i / 2880) }
  })
  const q = state.user?.quota ?? { memory_mb: 256, storage_mb: 1024 }
  return { quota: { memory_mb: q.memory_mb, storage_mb: q.storage_mb }, memory_bytes, storage_bytes, sources, history }
}

const logs: LogLine[] = [
  { ts: now() - 60000, level: 'info', source: 'core', message: '小鱼醒了，开始上课' },
  { ts: now() - 50000, level: 'info', source: 'qq_official', message: '已连上 QQ 网关，会话已恢复' },
  { ts: now() - 30000, level: 'info', source: 'plugin:daily_quote', message: '插件已加载，注册指令 /一言' },
  { ts: now() - 8000, level: 'warn', source: 'llm', message: '模型回复较慢，用时 6.2 秒' },
]

export async function mockFetch(method: string, path: string, body: any): Promise<Response> {
  await new Promise((r) => setTimeout(r, 180 + Math.random() * 220))
  const m = (re: RegExp) => path.match(re)
  if (path === '/captcha') {
    return ok({
      id: id(),
      svg: '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="44"><rect width="120" height="44" fill="#fff6e8"/><path d="M12 30 Q30 8 44 28 T76 26 T108 20" stroke="#26307a" stroke-width="2.4" fill="none"/><path d="M18 14 L30 34 M48 12 L56 34 M70 14 Q84 30 92 12 M98 30 L110 14" stroke="#e8706b" stroke-width="3" stroke-linecap="round" fill="none"/></svg>',
    })
  }
  if (path === '/auth/email-code') {
    if (body.purpose === 'register') {
      const inv = takeInvite(body.invite_code)
      if (inv instanceof Response) return inv
    }
    return ok({ ok: true, cooldown_s: 60 })
  }
  if (path === '/auth/register' || path === '/auth/login') {
    if (!body?.email || !body?.password) return err(400, 'invalid', '请填写邮箱和密码')
    if (body.password === 'wrong') return err(401, 'invalid_credentials', '邮箱或密码不对')
    let inv: Invite | null = null
    if (path === '/auth/register') {
      const r = takeInvite(body.invite_code)
      if (r instanceof Response) return r
      inv = r
    }
    state.user = seedUser(body.email)
    if (inv) {
      inv.used_count++
      inv.redemptions.push({ user_id: state.user.id, user_email: body.email, days: inv.days, redeemed_at: now() })
      state.user.service_until = now() + inv.days * 86400000
    }
    if (body.email.startsWith('expired')) Object.assign(state.user, { service_until: now() - 86400000, service_active: false })
    return ok({ user: state.user })
  }
  if (path === '/auth/logout') {
    state.user = null
    return ok({ ok: true })
  }
  if (path === '/auth/reset') return ok({ ok: true })
  if (!state.user) return err(401, 'unauthorized', '请先登录')
  if (path === '/me') return ok({ user: state.user })
  if (path === '/me/redeem') {
    const inv = takeInvite(body.code)
    if (inv instanceof Response) return inv
    inv.used_count++
    inv.redemptions.push({ user_id: state.user.id, user_email: state.user.email, days: inv.days, redeemed_at: now() })
    state.user.service_until = Math.max(now(), state.user.service_until ?? 0) + inv.days * 86400000
    state.user.service_active = true
    return ok({ user: state.user })
  }
  const blocked = () => err(403, 'service_expired', '服务已到期，兑换新的邀请码后再来')
  if (!state.user.service_active && method === 'POST' && (/^\/bots$/.test(path) || /\/(start|restart|plugins)$/.test(path))) return blocked()

  if (path === '/llm-keys' && method === 'GET') return ok({ items: state.keys })
  if (path === '/llm-keys' && method === 'POST') {
    const k: LlmKey = {
      id: id(),
      name: body.name,
      base_url: body.base_url,
      model: body.model,
      key_masked: `${String(body.api_key).slice(0, 3)}…${String(body.api_key).slice(-4)}`,
      created_at: now(),
    }
    state.keys.push(k)
    return ok(k)
  }
  let r = m(/^\/llm-keys\/(\w+)$/)
  if (r && method === 'DELETE') {
    state.keys = state.keys.filter((k) => k.id !== r![1])
    return ok({ ok: true })
  }

  if (path === '/bots' && method === 'GET') return ok({ items: state.bots })
  if (path === '/bots' && method === 'POST') {
    if (state.bots.length >= state.user.quota.bots) return err(403, 'quota_exceeded', '小鱼名额已经用完了')
    const b: Bot = {
      id: id(),
      name: body.name || '大肥鱼',
      avatar: null,
      status: 'starting',
      created_at: now(),
      updated_at: now(),
      settings: defaultSettings(),
      platforms: [],
      plugins: [],
    }
    state.bots.push(b)
    setRunning(b, 3000)
    return ok(b)
  }
  const findBot = (bid: string) => state.bots.find((b) => b.id === bid)
  r = m(/^\/bots\/(\w+)$/)
  if (r) {
    const b = findBot(r[1])
    if (!b) return err(404, 'not_found', '找不到这条小鱼')
    if (method === 'GET') return ok(b)
    if (method === 'PATCH') {
      Object.assign(b, body, { updated_at: now() })
      return ok(b)
    }
    if (method === 'DELETE') {
      state.bots = state.bots.filter((x) => x !== b)
      return ok({ ok: true })
    }
  }
  r = m(/^\/bots\/(\w+)\/(start|stop|restart)$/)
  if (r) {
    const b = findBot(r[1])
    if (!b) return err(404, 'not_found', '找不到这条小鱼')
    if (r[2] === 'stop') {
      b.status = 'stopped'
      b.status_detail = undefined
      pushLog(b.id, { level: 'info', source: 'core', message: '小鱼下课睡觉了' })
    } else {
      if (r[2] === 'restart') pushLog(b.id, { level: 'info', source: 'core', message: '收到重启指令' })
      setRunning(b, 2500)
    }
    return ok(b)
  }
  r = m(/^\/bots\/(\w+)\/logs/)
  if (r) {
    const b = findBot(r[1])
    if (!b) return err(404, 'not_found', '找不到这条小鱼')
    if (b.status === 'running' && Math.random() < 0.7) pushLog(b.id, logPool[Math.floor(Math.random() * logPool.length)])
    return ok({ items: [...(botLogs[b.id] ?? [])], cursor: String(now()) })
  }
  r = m(/^\/bots\/(\w+)\/platforms\/qq\/manual$/)
  if (r) {
    const b = findBot(r[1])!
    if (!/^\d{5,}$/.test(String(body.app_id)) || !body.secret) return err(400, 'invalid_input', 'AppID 或 AppSecret 不对，再核对一下')
    const p = {
      id: id(),
      kind: 'qq_official' as const,
      display_name: 'QQ 官方机器人',
      app_id: String(body.app_id),
      secret_masked: `${String(body.secret).slice(0, 3)}…${String(body.secret).slice(-4)}`,
      enabled: true,
      status: 'connected' as const,
    }
    b.platforms.push(p)
    return ok(p)
  }
  r = m(/^\/bots\/(\w+)\/platforms\/(qq|weixin)\/qr$/)
  if (r) {
    const t = id()
    state.qr[t] = now()
    const qr_url = r[2] === 'qq' ? `https://q.qq.com/qqbot/openclaw/connect.html?task_id=${t}` : `https://ilinkai.weixin.qq.com/connect?task=${t}`
    const ttl = mockDev.qrMode === 'expire' ? 8000 : 300000
    return ok({ task_id: t, qr_url, interval_s: 2, expires_at: now() + ttl })
  }
  r = m(/^\/bots\/(\w+)\/platforms\/(qq|weixin)\/qr\/(\w+)$/)
  if (r) {
    const started = state.qr[r[3]] ?? now()
    const b = findBot(r[1])!
    const elapsed = now() - started
    if (mockDev.qrMode === 'expire') return ok({ status: elapsed < 8000 ? 'pending' : 'expired' })
    if (mockDev.qrMode === 'error' && elapsed > 4000) return ok({ status: 'error', message: '这个 QQ 号创建机器人的次数到上限了，换个号再试' })
    if (elapsed < 6000) return ok({ status: 'pending' })
    if (r[2] === 'weixin' && elapsed < 9000) return ok({ status: 'scanned' })
    const kind = r[2] === 'qq' ? 'qq_official' : 'weixin_ilink'
    let p = b.platforms.find((x) => x.kind === kind)
    if (!p) {
      p = {
        id: id(),
        kind,
        display_name: kind === 'qq_official' ? 'QQ 官方机器人' : '微信',
        app_id: kind === 'qq_official' ? '102384756' : 'wxid_fish01',
        secret_masked: kind === 'qq_official' ? 'Xk3…9fQa' : '已保存',
        enabled: true,
        status: 'connected',
      }
      b.platforms.push(p)
    }
    return ok({ status: 'created', platform: p })
  }
  r = m(/^\/bots\/(\w+)\/platforms\/(\w+)$/)
  if (r) {
    const b = findBot(r[1])!
    if (method === 'DELETE') {
      b.platforms = b.platforms.filter((p) => p.id !== r![2])
      return ok({ ok: true })
    }
    const p = b.platforms.find((x) => x.id === r![2])!
    Object.assign(p, body)
    if ('enabled' in body) p.status = body.enabled ? 'connected' : 'disabled'
    return ok(p)
  }
  if (path === '/plugins/inspect') {
    const u = String(body.repo_url)
    if (!/^https:\/\/github\.com\/[\w.-]+\/[\w.-]+/.test(u)) return err(400, 'repo_invalid', '这不像是 GitHub 仓库地址，应该长这样：https://github.com/作者/仓库')
    const name = u.replace(/\/tree\/.*$/, '').split('/').filter(Boolean).pop()!.replace(/\.git$/, '')
    if (!/plugin/i.test(name)) return err(400, 'not_astrbot_plugin', '这个仓库里没找到 metadata.yaml，看起来不是 AstrBot 插件')
    return ok({
      name,
      display_name: name.replace(/^astrbot_plugin_/, '').replace(/_/g, ' '),
      version: 'v1.0.3',
      author: u.split('/')[3],
      description: '从仓库 metadata.yaml 读到的插件简介',
      repo_url: u,
      schema: samplePlugin(name, name, 20).schema,
      has_requirements: true,
    })
  }
  r = m(/^\/bots\/(\w+)\/plugins$/)
  if (r) {
    const b = findBot(r[1])!
    const name = String(body.repo_url).replace(/\/tree\/.*$/, '').split('/').filter(Boolean).pop()!.replace(/\.git$/, '')
    if (b.plugins.some((x) => x.name === name)) return err(409, 'plugin_exists', '这条小鱼已经装过这个插件了')
    const total = state.bots.reduce((n, x) => n + x.plugins.length, 0)
    if (total >= state.user.quota.plugins) return err(403, 'quota_exceeded', `插件额度是 ${state.user.quota.plugins} 个，已经用完了`)
    const p = samplePlugin(name, name.replace(/^astrbot_plugin_/, ''), 18 + Math.round(Math.random() * 30))
    p.repo_url = body.repo_url
    p.status = 'installing'
    b.plugins.push(p)
    setTimeout(() => {
      if (/heavy/.test(name)) Object.assign(p, { status: 'over_quota', status_detail: '内存超过额度（241 MB / 256 MB），插件先停下了' })
      else if (/broken/.test(name)) Object.assign(p, { status: 'error', status_detail: '加载失败：ModuleNotFoundError: No module named \'aiohttp_socks\'' })
      else p.status = 'running'
      if (b.status === 'running') pushLog(b.id, { level: p.status === 'running' ? 'info' : 'error', source: `plugin:${name}`, message: p.status_detail ?? '插件已安装并加载' })
    }, 5000)
    return ok(p)
  }
  r = m(/^\/bots\/(\w+)\/plugins\/(\w+)(\/update)?$/)
  if (r) {
    const b = findBot(r[1])!
    if (method === 'DELETE') {
      b.plugins = b.plugins.filter((p) => p.id !== r![2])
      return ok({ ok: true })
    }
    const p = b.plugins.find((x) => x.id === r![2])!
    if (r[3]) p.status = 'installing'
    else Object.assign(p, body, { updated_at: now() })
    if (r[3]) setTimeout(() => (p.status = 'running'), 3000)
    if ('enabled' in (body ?? {})) p.status = body.enabled ? 'running' : 'stopped'
    return ok(p)
  }
  if (path === '/usage') return ok(usage())
  if (path.startsWith('/admin')) {
    if (state.user.role !== 'admin') return err(403, 'forbidden', '只有管理员能看这里')
    return adminRoute(method, path, body)
  }
  return err(404, 'not_found', `mock: ${method} ${path}`)
}

const DAY = 86400000
const others: AdminUser[] = ['xiaoyu@example.com', 'qunzhu@example.com', 'astr.fan@example.com', 'newbie@example.com'].map(
  (email, i) => ({
    id: 'u' + i,
    email,
    role: 'user',
    created_at: now() - (i + 1) * 3 * DAY,
    quota: { memory_mb: 256, storage_mb: 1024, bots: 1, plugins: 8 },
    service_until: now() + [26, 3, 71, -2][i] * DAY,
    service_active: i !== 3,
    bots: i === 3 ? 0 : 1,
    plugins: [3, 6, 8, 0][i],
    running: i === 3 ? 0 : i === 1 ? 0 : 1,
    errors: i === 1 ? 1 : 0,
    memory_bytes: i === 3 ? 0 : [64, 141, 228, 0][i] * MB,
    storage_bytes: i === 3 ? 0 : [120, 310, 802, 0][i] * MB,
    last_login_at: now() - (i + 1) * 5 * 3600000,
  }),
)

const filler: AdminUser[] = Array.from({ length: 56 }, (_, i) => ({
  id: 'f' + i,
  email: `student${String(i + 1).padStart(2, '0')}@example.com`,
  role: 'user',
  created_at: now() - (20 + i) * DAY,
  quota: { memory_mb: 256, storage_mb: 1024, bots: 1, plugins: 8 },
  service_until: now() + ((i % 9) * 11 - 20) * DAY,
  service_active: (i % 9) * 11 - 20 > 0,
  bots: 0,
  plugins: 0,
  running: 0,
  errors: 0,
  memory_bytes: i % 7 === 0 ? null : 0,
  storage_bytes: i % 7 === 0 ? null : 0,
  last_login_at: i % 5 === 0 ? null : now() - (i + 2) * DAY,
}))

function selfAdminUser(): AdminUser {
  const u = usage()
  return {
    ...state.user!,
    bots: state.bots.length,
    plugins: state.bots.reduce((n, b) => n + b.plugins.length, 0),
    running: state.bots.filter((b) => b.status === 'running').length,
    errors: state.bots.filter((b) => b.status === 'error').length,
    memory_bytes: u.memory_bytes,
    storage_bytes: u.storage_bytes,
    last_login_at: now(),
  }
}

function adminRoute(method: string, path: string, body: any): Response {
  const all = [selfAdminUser(), ...others, ...filler]
  if (path === '/admin/overview') {
    const o: AdminOverview = {
      users: { total: all.length, new_7d: 3, active: all.filter((u) => u.service_active).length, expiring_7d: 1, expired: 1 },
      invites: { unused: invites.filter((i) => !i.revoked_at && i.used_count < i.max_uses).length, used_7d: 2 },
      bots: { total: 4, running: 3, starting: 0, stopped: 0, error: 1 },
      plugins: { total: 17, running: 15, installing: 0, error: 1, over_quota: 1 },
      platforms: { qq_official: 3, weixin_ilink: 1, error: 1 },
      usage: { memory_bytes: 433 * MB, storage_bytes: 1232 * MB, quota_memory_mb: 256 * all.length, quota_storage_mb: 1024 * all.length },
      nodes: [
        { id: 'node-1', base_url: 'https://node-1.fish.niox.lol', online: true, version: '0.1.0', memory_total_bytes: 1950 * MB, memory_used_bytes: 812 * MB, bots: 4, last_seen: now() - 20000 },
      ],
      problems: [
        { user_id: 'u1', user_email: 'qunzhu@example.com', bot_id: 'b1', bot_name: '群聊小鱼', kind: 'platform', id: 'p1', label: 'QQ 官方机器人', status: 'error', status_detail: 'AppSecret 已失效，请重新扫码', updated_at: now() - 40 * 60000 },
        { user_id: 'u2', user_email: 'astr.fan@example.com', bot_id: 'b2', bot_name: 'Astr 搬家', kind: 'plugin', id: 'pl2', label: 'meme_manager', status: 'over_quota', status_detail: '内存超过额度（228 MB / 256 MB）', updated_at: now() - 2 * 3600000 },
      ],
    }
    return ok(o)
  }
  if (path.startsWith('/admin/users') && method === 'GET' && !/^\/admin\/users\/\w+/.test(path)) {
    const sp = new URLSearchParams(path.split('?')[1] ?? '')
    const q = sp.get('q') ?? ''
    const from = Number(sp.get('cursor') ?? 0)
    const hits = all.filter((u) => u.email.includes(q))
    const next = from + 50 < hits.length ? String(from + 50) : null
    return ok({ items: hits.slice(from, from + 50), cursor: next })
  }
  let r = path.match(/^\/admin\/users\/(\w+)$/)
  if (r) {
    const u = all.find((x) => x.id === r![1])
    if (!u) return err(404, 'not_found', '找不到这个用户')
    if (method === 'PATCH') {
      const target: User | AdminUser = u.id === state.user!.id ? state.user! : u
      for (const x of target === u ? [u] : [u, target]) {
        if (body.quota) x.quota = { ...x.quota, ...body.quota }
        if ('service_until' in body) x.service_until = body.service_until
        if (body.extend_days) x.service_until = Math.max(now(), x.service_until ?? 0) + body.extend_days * DAY
        x.service_active = x.role === 'admin' || (x.service_until ?? 0) > now()
      }
      return ok(u)
    }
    const bots = u.id === state.user!.id ? state.bots : u.bots ? [{ ...demoBot(u), id: 'b' + u.id }] : []
    return ok({ user: u, bots })
  }
  r = path.match(/^\/admin\/users\/(\w+)\/usage$/)
  if (r) {
    if (r[1] === state.user!.id) return ok(usage())
    const u = all.find((x) => x.id === r![1])!
    if (u.memory_bytes === null) return err(502, 'node_unreachable', '节点暂时连不上，用量稍后再看')
    const base = usage()
    const mem = u.memory_bytes ?? 0
    return ok({
      ...base,
      quota: { memory_mb: u.quota.memory_mb, storage_mb: u.quota.storage_mb },
      memory_bytes: mem,
      storage_bytes: u.storage_bytes ?? 0,
      sources: u.bots
        ? [
            { bot_id: 'b' + u.id, kind: 'core', plugin_id: null, label: '核心连接', memory_bytes: 9.6 * MB, storage_bytes: 0.2 * MB, cpu_percent: 0.4 },
            { bot_id: 'b' + u.id, kind: 'conversations', plugin_id: null, label: '聊天记录', memory_bytes: 0, storage_bytes: 12 * MB, cpu_percent: 0 },
            { bot_id: 'b' + u.id, kind: 'plugin', plugin_id: 'x', label: 'meme_manager', memory_bytes: mem - 9.6 * MB, storage_bytes: (u.storage_bytes ?? 0) - 12.2 * MB, cpu_percent: 2.1 },
          ]
        : [],
      history: base.history.map((h) => ({ ...h, memory_bytes: mem * (0.8 + Math.random() * 0.1), storage_bytes: u.storage_bytes ?? 0 })),
    })
  }
  if (/^\/admin\/users\/\w+\/bots\/\w+\/logs/.test(path)) return ok({ items: logs, cursor: String(now()) })
  if (path.startsWith('/admin/invites') && method === 'GET') {
    const st = new URLSearchParams(path.split('?')[1] ?? '').get('status') ?? 'all'
    const items = invites.filter((i) =>
      st === 'all' ? true : st === 'revoked' ? !!i.revoked_at : st === 'used' ? !i.revoked_at && i.used_count >= i.max_uses : !i.revoked_at && i.used_count < i.max_uses,
    )
    return ok({ items: [...items].sort((a, b) => b.created_at - a.created_at), cursor: null })
  }
  if (path === '/admin/invites' && method === 'POST') {
    const items: Invite[] = Array.from({ length: body.count ?? 1 }, () => ({
      code: genCode(), days: body.days, max_uses: body.max_uses ?? 1, used_count: 0, note: body.note || null,
      created_by_email: state.user!.email, created_at: now(), expires_at: body.expires_at ?? null, revoked_at: null, redemptions: [],
    }))
    invites.push(...items)
    return ok({ items })
  }
  r = path.match(/^\/admin\/invites\/(.+)$/)
  if (r && method === 'DELETE') {
    const inv = invites.find((i) => i.code === decodeURIComponent(r![1]))
    if (inv) inv.revoked_at = now()
    return ok({ ok: true })
  }
  return err(404, 'not_found', `mock: ${method} ${path}`)
}

function demoBot(u: AdminUser): Bot {
  return {
    id: 'b' + u.id,
    name: u.errors ? '群聊小鱼' : '大肥鱼',
    avatar: null,
    status: u.running ? 'running' : 'error',
    status_detail: u.errors ? 'QQ 官方机器人连接失败' : undefined,
    created_at: u.created_at,
    updated_at: now(),
    settings: defaultSettings(),
    platforms: [
      { id: 'p' + u.id, kind: 'qq_official', display_name: 'QQ 官方机器人', app_id: '10238' + u.id.slice(1), secret_masked: 'Xk3…9fQa', enabled: true, status: u.errors ? 'error' : 'connected', status_detail: u.errors ? 'AppSecret 已失效，请重新扫码' : undefined },
    ],
    plugins: Array.from({ length: u.plugins }, (_, i) => ({ ...samplePlugin('astrbot_plugin_demo' + i, '示例插件 ' + (i + 1), 12), status: 'running' as const })),
  }
}
