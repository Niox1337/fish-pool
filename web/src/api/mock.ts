import type {
  AdminCredit,
  AdminOverview,
  AdminSystem,
  AdminUser,
  Bot,
  Invite,
  LlmKey,
  LogLine,
  Plugin,
  QqPanelRecord,
  Quota,
  Usage,
  ConversationSession,
  User,
} from './types'

const MB = 1024 * 1024
const defaultQuota: Quota = { memory_mb: 128, storage_mb: 128, bots: null, plugins: null }
const now = () => Date.now()
const id = () => Math.random().toString(36).slice(2, 12)

const defaultSettings = (): Bot['settings'] => ({
  persona: { name: '大肥鱼', system_prompt: '你是一条爱吃白饭的蓝色大肥鱼，说话软乎乎的，喜欢帮群友解答问题。', greeting: '' },
  llm: { key_id: null, model: '', temperature: 0.7, max_tokens: 1024, context_turns: 12 },
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

/** Gateway credits by user id. */
const credits: Record<string, AdminCredit[]> = {
  u2: [
    {
      id: 'c1',
      source: 'invite',
      amount: '5.00',
      expires_at: now() + 3 * 86400000,
      expired: false,
      note: '',
      created_at: now() - 27 * 86400000,
      created_by_email: null,
      state: 'granted',
      error: null,
      spent: '1.2380',
      remaining: '3.7620',
    },
    {
      id: 'c2',
      source: 'admin',
      amount: '10.00',
      expires_at: null,
      expired: false,
      note: '插件测试补偿',
      created_at: now() - 2 * 86400000,
      created_by_email: 'admin@fish.niox.lol',
      state: 'granted',
      error: null,
      spent: '0.0000',
      remaining: '10.0000',
    },
  ],
}
const yuan2 = (v: string | number) => Number(v).toFixed(2)
const yuan4 = (v: string | number) => Number(v).toFixed(4)
const isoOf = (ms: number | null) => (ms == null ? null : new Date(ms).toISOString())

function addMockCredit(userId: string, c: Omit<AdminCredit, 'id' | 'expired' | 'created_at' | 'state' | 'error' | 'spent' | 'remaining'>) {
  ;(credits[userId] ??= []).unshift({
    ...c,
    id: id(),
    expired: false,
    created_at: now(),
    state: 'granted',
    error: null,
    spent: '0.0000',
    remaining: yuan4(c.amount),
  })
  if (userId === state.user?.id && !state.keys.some((k) => k.rotatable)) {
    state.keys.push({
      id: id(),
      name: '活动获取',
      base_url: 'https://gateway.gengwiki.com/v1',
      model: 'deepseek-flash',
      key_masked: 'gwk…3f2a',
      created_at: now(),
      has_balance: true,
      rotatable: true,
    })
  }
}

function mockBalance(userId: string) {
  const live = (credits[userId] ?? [])
    .filter((c) => c.expires_at == null || c.expires_at > now())
    .sort((a, b) => (a.expires_at ?? Infinity) - (b.expires_at ?? Infinity))
  const list = live.map((c) => ({
    available: yuan4(c.remaining ?? c.amount),
    amount: yuan2(c.amount),
    spent: yuan4(c.spent ?? 0),
    expires_at: isoOf(c.expires_at),
  }))
  const total = live.reduce((n, c) => n + Number(c.remaining ?? c.amount), 0)
  return { currency: 'CNY', available: yuan4(total), budget: '0.00', spent: '0.0000', expires_at: null, disabled: false, credits: list }
}

const state: State = {
  user: null,
  keys: [],
  bots: [],
  qr: {},
}

const botLogs: Record<string, LogLine[]> = {}
const qqPanelRecords = new Map<string, QqPanelRecord[]>()
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
    pushLog(b.id, { level: 'info', source: 'core', message: '小鱼已开始运行' })
    for (const p of b.platforms) pushLog(b.id, { level: 'info', source: p.kind, message: `已连上${p.display_name}` })
    for (const p of b.plugins) pushLog(b.id, { level: 'info', source: `plugin:${p.name}`, message: '插件已加载' })
  }, delay)
}

const gatewayBaseUrls = [
  'https://api.openai.com/v1',
  'https://api.anthropic.com/v1',
  'https://generativelanguage.googleapis.com/v1beta/openai',
  'https://api.x.ai/v1',
]

const mockDev = {
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
  config_revision: id(),
  schema: {
    daily_limit: { type: 'int', description: '每人每天最多触发次数', default: 5 },
    nickname: { type: 'string', description: '插件里怎么称呼群友', hint: '留空就用群名片', default: '' },
    enable_image: { type: 'bool', description: '回复时附带图片', default: true },
    image_size: {
      type: 'string',
      description: '图片尺寸',
      options: ['small', 'large'],
      labels: ['小图', '大图'],
      default: 'small',
      condition: { enable_image: true },
    },
    style: { type: 'string', description: '语气', options: ['可爱', '正经', '毒舌'], default: '可爱' },
    chance: { type: 'float', description: '随机接话概率', slider: { min: 0, max: 1, step: 0.05 }, default: 0.1 },
    intro: { type: 'text', description: '自我介绍', hint: '有人发 /介绍 时回复这段话', obvious_hint: true, default: '' },
    keywords: { type: 'list', description: '触发关键词', default: ['早安', '晚安'] },
    session_list: {
      type: 'list',
      description: '私聊会话 UMO 列表',
      hint: '格式: {platform_name}:FriendMessage:{session_id}，可通过 /sid 指令获取',
      default: [],
    },
    admin_users: { type: 'list', description: '插件管理员', default: [] },
    api: {
      type: 'object',
      description: '图源接口',
      items: {
        endpoint: { type: 'string', description: '接口地址', default: 'https://api.example.com' },
        token: { type: 'string', description: '访问令牌', secret: true, default: '' },
        timeout: { type: 'int', description: '超时秒数', default: 10 },
      },
    },
    templates: {
      type: 'template_list',
      description: '回复模板',
      templates: {
        fixed: {
          name: '固定回复',
          display_item: 'name',
          items: {
            name: { type: 'string', description: '模板名称' },
            text: { type: 'text', description: '回复内容' },
          },
        },
      },
      default: [{ __template_key: 'fixed', name: '默认', text: '今天也要好好吃饭' }],
    },
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
    quota: { ...defaultQuota },
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
  {
    code: 'FISH-7K2M-Q9TD',
    days: 30,
    max_uses: 1,
    used_count: 0,
    note: '群友试用',
    gateway_budget: '5.00',
    created_by_email: 'admin@fish.niox.lol',
    created_at: now() - 86400000,
    expires_at: null,
    revoked_at: null,
    redemptions: [],
  },
  {
    code: 'FISH-9WQE-2RJA',
    days: 7,
    max_uses: 5,
    used_count: 2,
    note: '活动群批量',
    created_by_email: 'admin@fish.niox.lol',
    created_at: now() - 9 * 86400000,
    expires_at: now() - 2 * 86400000,
    revoked_at: now() - 3 * 86400000,
    redemptions: [
      { user_id: 'u0', user_email: 'xiaoyu@example.com', days: 7, redeemed_at: now() - 8 * 86400000 },
      { user_id: 'u1', user_email: 'qunzhu@example.com', days: 7, redeemed_at: now() - 6 * 86400000 },
    ],
  },
  {
    code: 'FISH-3HXV-PB6N',
    days: 90,
    max_uses: 1,
    used_count: 1,
    note: null,
    created_by_email: 'admin@fish.niox.lol',
    created_at: now() - 5 * 86400000,
    expires_at: null,
    revoked_at: null,
    redemptions: [{ user_id: 'u2', user_email: 'astr.fan@example.com', days: 90, redeemed_at: now() - 4 * 86400000 }],
  },
].map((invite) => ({ ...invite, quota: null }))
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

const conversations = new Map<string, ConversationSession[]>()
const clearedPlugins = new Set<string>()

function mockSessions(botId: string, platformId: string): ConversationSession[] {
  let list = conversations.get(botId)
  if (!list) {
    list = [
      { session: `${platformId}:GroupMessage:739215804`, messages: 200, bytes: 1.4 * MB, last_ts: now() - 3 * 60_000 },
      { session: `${platformId}:GroupMessage:118830276`, messages: 146, bytes: 0.9 * MB, last_ts: now() - 2 * 3600_000 },
      { session: `${platformId}:FriendMessage:2108511981`, messages: 58, bytes: 0.3 * MB, last_ts: now() - 26 * 3600_000 },
      { session: `gone:FriendMessage:95501123`, messages: 12, bytes: 0.05 * MB, last_ts: now() - 9 * 86400_000 },
    ]
    conversations.set(botId, list)
  }
  return list
}

function usage(): Usage {
  const sources: Usage['sources'] = []
  for (const b of state.bots) {
    sources.push({
      bot_id: b.id,
      kind: 'core',
      plugin_id: null,
      label: '核心连接',
      memory_bytes: 9.6 * MB,
      storage_bytes: 0.2 * MB,
      cpu_percent: 0.4,
    })
    sources.push({
      bot_id: b.id,
      kind: 'conversations',
      plugin_id: null,
      label: '聊天记录',
      memory_bytes: 0,
      storage_bytes: (conversations.get(b.id) ?? mockSessions(b.id, b.platforms[0]?.id ?? 'p1')).reduce((n, x) => n + x.bytes, 0),
      cpu_percent: 0,
    })
    for (const p of b.plugins) {
      const mem = ((p as unknown as { _mem?: number })._mem ?? 24) * MB
      sources.push({
        bot_id: b.id,
        kind: 'plugin',
        plugin_id: p.id,
        label: p.display_name,
        memory_bytes: p.enabled ? mem : 0,
        storage_bytes: clearedPlugins.has(p.id) ? 4 * 1024 : mem * 0.12,
        deps_bytes: mem * 3.2,
        cpu_percent: p.enabled ? 1.2 : 0,
      })
    }
  }
  const memory_bytes = sources.reduce((s, x) => s + x.memory_bytes, 0)
  const storage_bytes = sources.reduce((s, x) => s + x.storage_bytes, 0)
  const history = Array.from({ length: 288 }, (_, i) => {
    const t = now() - (287 - i) * 5 * 60 * 1000
    const wave = Math.sin(i / 18) * 0.12 + Math.sin(i / 5) * 0.04
    return { ts: t, memory_bytes: Math.max(0, memory_bytes * (0.82 + wave)), storage_bytes: storage_bytes * (0.9 + i / 2880) }
  })
  const q = state.user?.quota ?? defaultQuota
  return { quota: { memory_mb: q.memory_mb, storage_mb: q.storage_mb }, memory_bytes, storage_bytes, sources, history }
}

const logs: LogLine[] = [
  { ts: now() - 60000, level: 'info', source: 'core', message: '小鱼已开始运行' },
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
      if (inv.quota) state.user.quota = { ...inv.quota }
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
  if (path === '/plugins/market' && method === 'GET') {
    return ok({
      items: [
        ['Niox1337', 'astrbot_plugin_gengwiki', '梗Wiki', '让小鱼跟上群里的新梗，顺手回一张表情包。', '娱乐'],
        ['example', 'astrbot_plugin_notes', '小鱼便签', '把群里的提醒和待办保存下来，随时用指令查看。', '工具'],
        ['example', 'astrbot_plugin_knowledge', '群聊资料库', '上传资料后，按问题从资料中检索相关内容。', '知识库'],
      ].map(([author, name, display_name, description, category]) => ({
        id: author + '/' + name,
        author,
        name,
        display_name,
        description,
        category,
        repo_url: 'https://github.com/' + author + '/' + name,
        tags: [],
      })),
    })
  }
  if (path === '/me/redeem') {
    const inv = takeInvite(body.code)
    if (inv instanceof Response) return inv
    inv.used_count++
    inv.redemptions.push({ user_id: state.user.id, user_email: state.user.email, days: inv.days, redeemed_at: now() })
    state.user.service_until = Math.max(now(), state.user.service_until ?? 0) + inv.days * 86400000
    state.user.service_active = true
    if (inv.gateway_budget) {
      addMockCredit(state.user.id, {
        source: 'invite',
        amount: inv.gateway_budget,
        expires_at: state.user.service_until,
        note: '',
        created_by_email: null,
      })
    }
    return ok({ user: state.user })
  }
  const blocked = () => err(403, 'service_expired', '服务已到期，兑换新的邀请码后再来')
  if (!state.user.service_active && method === 'POST' && (/^\/bots$/.test(path) || /\/(start|restart|plugins)$/.test(path)))
    return blocked()

  if (path === '/llm-keys' && method === 'GET') return ok({ items: state.keys })
  if (path === '/llm-keys' && method === 'POST') {
    const k: LlmKey = {
      id: id(),
      name: body.name,
      base_url: body.base_url,
      model: body.model,
      key_masked: `${String(body.api_key).slice(0, 3)}…${String(body.api_key).slice(-4)}`,
      created_at: now(),
      via_gateway: gatewayBaseUrls.includes(String(body.base_url).replace(/\/+$/, '')),
      has_balance: String(body.base_url).replace(/\/+$/, '') === 'https://gateway.gengwiki.com/v1',
    }
    state.keys.push(k)
    return ok(k)
  }
  if (path === '/me/gateway' && method === 'GET') {
    const k = state.keys.find((x) => x.rotatable) ?? null
    const has = (credits[state.user.id] ?? []).length > 0
    return ok({ account: has, key: k, balance: k ? mockBalance(state.user.id) : null, pending: [] })
  }
  if (path === '/me/gateway/rotate' && method === 'POST') {
    let k = state.keys.find((x) => x.rotatable)
    if (!(credits[state.user.id] ?? []).length) return err(404, 'not_found', '还没有活动额度')
    if (!k) {
      k = {
        id: id(),
        name: '活动获取',
        base_url: 'https://gateway.gengwiki.com/v1',
        model: 'deepseek-flash',
        key_masked: '',
        created_at: now(),
        has_balance: true,
        rotatable: true,
      }
      state.keys.push(k)
    }
    k.key_masked = `gwk…${Math.random().toString(16).slice(2, 6)}`
    return ok({ key: k })
  }
  const bal = m(/^\/llm-keys\/(\w+)\/balance$/)
  if (bal && method === 'GET') return ok(mockBalance(state.user.id))
  let r = m(/^\/llm-keys\/(\w+)$/)
  if (r && method === 'DELETE') {
    state.keys = state.keys.filter((k) => k.id !== r![1])
    return ok({ ok: true })
  }

  if (path === '/bots' && method === 'GET') return ok({ items: state.bots })
  if (path === '/bots' && method === 'POST') {
    if (state.user.quota.bots != null && state.bots.length >= state.user.quota.bots) return err(403, 'quota_exceeded', '小鱼名额已经用完了')
    if (!body.key_id) return err(400, 'invalid_input', '请选择模型 Key')
    const model = typeof body.model === 'string' ? body.model.trim() : ''
    if (!model || [...model].length > 100) return err(400, 'invalid_input', '请选择有效的模型')
    if (!state.keys.some((key) => key.id === body.key_id)) return err(404, 'not_found', '所选的模型 Key 不存在')
    const settings = defaultSettings()
    settings.persona.name = body.name || '大肥鱼'
    settings.llm.key_id = body.key_id
    settings.llm.model = model
    const b: Bot = {
      id: id(),
      name: body.name || '大肥鱼',
      avatar: null,
      status: 'starting',
      created_at: now(),
      updated_at: now(),
      settings,
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
      mode: 'websocket' as const,
    }
    b.platforms.push(p)
    return ok(p)
  }
  r = m(/^\/bots\/(\w+)\/platforms\/(qq|weixin)\/qr$/)
  if (r) {
    const t = id()
    state.qr[t] = now()
    const qr_url =
      r[2] === 'qq' ? `https://q.qq.com/qqbot/openclaw/connect.html?task_id=${t}` : `https://ilinkai.weixin.qq.com/connect?task=${t}`
    const ttl = mockDev.qrMode === 'expire' ? 8000 : 300000
    return ok({ task_id: t, qr_url, interval_s: 2, expires_at: now() + ttl })
  }
  r = m(/^\/bots\/(\w+)\/platforms\/(qq|weixin)\/qr\/(\w+)$/)
  if (r) {
    const started = state.qr[r[3]] ?? now()
    const b = findBot(r[1])!
    const elapsed = now() - started
    if (mockDev.qrMode === 'expire') return ok({ status: elapsed < 8000 ? 'pending' : 'expired' })
    if (mockDev.qrMode === 'error' && elapsed > 4000)
      return ok({ status: 'error', message: '这个 QQ 号创建机器人的次数到上限了，换个号再试' })
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
        mode: kind === 'qq_official' ? 'websocket' : undefined,
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
    if ('mode' in body && p.enabled) {
      p.status = body.mode === 'webhook' ? 'connecting' : 'connected'
      p.status_detail = body.mode === 'webhook' ? '等待 QQ 开放平台回调验证' : undefined
    }
    return ok(p)
  }
  r = m(/^\/bots\/(\w+)\/platforms\/(\w+)\/qq-panels(?:\/([\w-]+))?(?:\?scope=(group|c2c))?$/)
  if (r) {
    const platform = findBot(r[1])?.platforms.find((entry) => entry.id === r![2] && entry.kind === 'qq_official')
    if (!platform) return err(404, 'not_found', 'QQ 接入不存在')
    const records = qqPanelRecords.get(platform.id) ?? []
    if (method === 'GET') return ok({ records: records.filter((record) => record.scope === r![4]) })
    if (method === 'POST') {
      const panel_id = 'p_' + id()
      qqPanelRecords.set(platform.id, [{ panel_id, scope: body.scope, target_type: 'all', panel: body.panel, version: 1 }, ...records])
      return ok({ panel_id })
    }
    const record = records.find((item) => item.panel_id === r![3])
    if (!record) return err(404, 'not_found', '指令面板不存在')
    if (method === 'DELETE') {
      qqPanelRecords.set(
        platform.id,
        records.filter((item) => item !== record),
      )
      return ok({ ok: true })
    }
    if (body.expected_version !== undefined && record.version !== body.expected_version) {
      return err(409, 'qq_panel_changed', 'QQ 上的面板已有更新，请重新载入后再编辑')
    }
    record.panel = body.panel
    record.version = (record.version ?? 0) + 1
    return ok({ version: record.version })
  }
  r = m(/^\/bots\/(\w+)\/plugins$/)
  if (r) {
    const b = findBot(r[1])!
    const [author, name] = String(body.plugin_id).split('/')
    if (!author || !name) return err(404, 'market_plugin_not_found', '插件市场里没有这个插件')
    if (b.plugins.some((x) => x.name === name)) return err(409, 'plugin_exists', '这条小鱼已经装过这个插件了')
    const total = state.bots.reduce((n, x) => n + x.plugins.length, 0)
    if (state.user.quota.plugins != null && total >= state.user.quota.plugins)
      return err(403, 'quota_exceeded', `插件额度是 ${state.user.quota.plugins} 个，已经用完了`)
    const p = samplePlugin(name, name.replace(/^astrbot_plugin_/, ''), 18 + Math.round(Math.random() * 30))
    if (name === 'astrbot_plugin_gengwiki') p.display_name = '梗Wiki'
    p.repo_url = 'https://github.com/' + author + '/' + name
    p.author = author
    p.status = 'installing'
    b.plugins.push(p)
    setTimeout(() => {
      if (/heavy/.test(name)) Object.assign(p, { status: 'over_quota', status_detail: '内存超过额度（241 MB / 256 MB），插件先停下了' })
      else if (/broken/.test(name))
        Object.assign(p, { status: 'error', status_detail: "加载失败：ModuleNotFoundError: No module named 'aiohttp_socks'" })
      else p.status = 'running'
      if (b.status === 'running')
        pushLog(b.id, {
          level: p.status === 'running' ? 'info' : 'error',
          source: `plugin:${name}`,
          message: p.status_detail ?? '插件已安装并加载',
        })
    }, 5000)
    return ok(p)
  }
  r = m(/^\/bots\/(\w+)\/recent$/)
  if (r) {
    const b = findBot(r[1])!
    const pid = b.platforms[0]?.id ?? 'p1'
    const friend = `${pid}:FriendMessage:5FCD19F6930E90B91C40F2E24A5C40AD`
    const group = `${pid}:GroupMessage:C1A7E0D29B3F4A6E8D5C2B1A09F8E7D6`
    return ok({
      sessions: [
        { id: friend, origin: friend, last_ts: now() - 4 * 60_000 },
        { id: group, origin: group, last_ts: now() - 52 * 60_000 },
      ],
      users: [
        { id: '5FCD19F6930E90B91C40F2E24A5C40AD', origin: friend, last_ts: now() - 4 * 60_000 },
        { id: '8B2E6F1A4C9D3E7B5A0F2C8D6E1B4A93', origin: group, last_ts: now() - 52 * 60_000 },
      ],
    })
  }
  r = m(/^\/bots\/(\w+)\/conversations(?:\?.*)?$/)
  if (r) {
    const b = findBot(r[1])!
    const list = mockSessions(b.id, b.platforms[0]?.id ?? 'p1')
    if (method === 'DELETE') {
      const session = new URL(path, 'http://x').searchParams.get('session')
      conversations.set(b.id, session ? list.filter((x) => x.session !== session) : [])
      return ok({ ok: true })
    }
    return ok({ items: list })
  }
  r = m(/^\/bots\/(\w+)\/plugins\/(\w+)\/clear-data$/)
  if (r) {
    const b = findBot(r[1])!
    const p = b.plugins.find((x) => x.id === r![2])
    if (!p) return err(404, 'not_found', '插件不存在')
    clearedPlugins.add(p.id)
    pushLog(b.id, { level: 'info', source: `plugin:${p.name}`, message: '插件数据已清除' })
    return ok({ ok: true })
  }
  r = m(/^\/bots\/(\w+)\/plugins\/(\w+)(?:\/(update|reload))?$/)
  if (r) {
    const b = findBot(r[1])!
    if (method === 'DELETE') {
      b.plugins = b.plugins.filter((p) => p.id !== r![2])
      return ok({ ok: true })
    }
    const p = b.plugins.find((x) => x.id === r![2])!
    if (r[3] === 'reload') {
      if (!p.enabled || b.status === 'stopped' || p.status === 'installing' || p.status === 'over_quota') {
        return err(409, 'plugin_reload_unavailable', '插件当前无法重载')
      }
      pushLog(b.id, { level: 'info', source: `plugin:${p.name}`, message: '插件已重载' })
      return ok({ ok: true })
    }
    if (r[3]) p.status = 'installing'
    else {
      if (body.config_revision && body.config_revision !== p.config_revision) {
        return err(409, 'plugin_config_changed', '插件设置有更新，请重新载入后再编辑')
      }
      if ('config' in body) {
        p.config = body.config
        p.config_revision = id()
      }
      p.updated_at = now()
    }
    if (r[3]) setTimeout(() => (p.status = 'running'), 3000)
    if ('enabled' in (body ?? {})) {
      p.enabled = body.enabled
      p.status = body.enabled ? 'running' : 'stopped'
    }
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
const others: AdminUser[] = ['xiaoyu@example.com', 'qunzhu@example.com', 'astr.fan@example.com', 'newbie@example.com'].map((email, i) => ({
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
}))

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
      plugins: { total: 17, running: 13, sleeping: 2, installing: 0, error: 1, over_quota: 1 },
      platforms: { qq_official: 3, weixin_ilink: 1, error: 1 },
      usage: { memory_bytes: 433 * MB, storage_bytes: 1232 * MB, quota_memory_mb: 256 * all.length, quota_storage_mb: 1024 * all.length },
      plugin_cpu: {
        percent: 43.5,
        reporting_nodes: 1,
        items: [
          {
            node_id: 'node-1',
            sampled_at: now() - 20_000,
            user_id: 'u2',
            bot_id: 'b2',
            bot_name: 'Astr 搬家',
            plugin_id: 'pl2',
            label: 'meme_manager',
            cpu_percent: 32.4,
          },
          {
            node_id: 'node-1',
            sampled_at: now() - 20_000,
            user_id: 'u1',
            bot_id: 'b1',
            bot_name: '群聊小鱼',
            plugin_id: 'pl1',
            label: 'daily_quote',
            cpu_percent: 11.1,
          },
        ],
      },
      nodes: [
        {
          id: 'node-1',
          base_url: 'https://fish-node-1.niox.lol',
          online: true,
          version: '0.1.0',
          memory_total_bytes: 4096 * MB,
          memory_used_bytes: 812 * MB,
          bots: 4,
          last_seen: now() - 20000,
        },
      ],
      problems: [
        {
          user_id: 'u1',
          user_email: 'qunzhu@example.com',
          bot_id: 'b1',
          bot_name: '群聊小鱼',
          kind: 'platform',
          id: 'p1',
          label: 'QQ 官方机器人',
          status: 'error',
          status_detail: 'AppSecret 已失效，请重新扫码',
          updated_at: now() - 40 * 60000,
        },
        {
          user_id: 'u2',
          user_email: 'astr.fan@example.com',
          bot_id: 'b2',
          bot_name: 'Astr 搬家',
          kind: 'plugin',
          id: 'pl2',
          label: 'meme_manager',
          status: 'over_quota',
          status_detail: '内存超过额度（228 MB / 256 MB）',
          updated_at: now() - 2 * 3600000,
        },
      ],
    }
    return ok(o)
  }
  if (path === '/admin/system') return ok(mockSystem())
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
  r = path.match(/^\/admin\/users\/(\w+)\/credits(?:\/(\w+))?$/)
  if (r) {
    const uid = r[1]
    const view = () => ({ configured: true, account: (credits[uid] ?? []).length > 0, items: credits[uid] ?? [], remote_error: null })
    if (method === 'POST') {
      if (!(Number(body.amount) > 0 && Number(body.amount) <= 1000))
        return err(400, 'invalid_input', '额度应为 0.01 到 1000 元，最多两位小数')
      if (body.expires_at != null && body.expires_at <= now()) return err(400, 'invalid_input', '过期时间应晚于现在')
      addMockCredit(uid, {
        source: 'admin',
        amount: yuan2(body.amount),
        expires_at: body.expires_at ?? null,
        note: body.note ?? '',
        created_by_email: state.user!.email,
      })
      return new Response(JSON.stringify(view()), { status: 201 })
    }
    if (method === 'PATCH' && r[2]) {
      const c = (credits[uid] ?? []).find((x) => x.id === r![2])
      if (!c) return err(404, 'not_found', '额度不存在')
      if (body.amount != null) {
        if (Number(body.amount) < Number(c.spent ?? 0)) return err(502, 'upstream_error', '修改失败：额度不能低于已用')
        c.amount = yuan2(body.amount)
        c.remaining = yuan4(Number(body.amount) - Number(c.spent ?? 0))
      }
      if ('expires_at' in body) {
        c.expires_at = body.expires_at
        c.expired = c.expires_at != null && c.expires_at <= now()
      }
      return ok(view())
    }
    return ok(view())
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
            {
              bot_id: 'b' + u.id,
              kind: 'core',
              plugin_id: null,
              label: '核心连接',
              memory_bytes: 9.6 * MB,
              storage_bytes: 0.2 * MB,
              cpu_percent: 0.4,
            },
            {
              bot_id: 'b' + u.id,
              kind: 'conversations',
              plugin_id: null,
              label: '聊天记录',
              memory_bytes: 0,
              storage_bytes: 12 * MB,
              cpu_percent: 0,
            },
            {
              bot_id: 'b' + u.id,
              kind: 'plugin',
              plugin_id: 'x',
              label: 'meme_manager',
              memory_bytes: mem - 9.6 * MB,
              storage_bytes: (u.storage_bytes ?? 0) - 12.2 * MB,
              cpu_percent: 2.1,
            },
          ]
        : [],
      history: base.history.map((h) => ({ ...h, memory_bytes: mem * (0.8 + Math.random() * 0.1), storage_bytes: u.storage_bytes ?? 0 })),
    })
  }
  if (/^\/admin\/users\/\w+\/bots\/\w+\/logs/.test(path)) return ok({ items: logs, cursor: String(now()) })
  if (path.startsWith('/admin/invites') && method === 'GET') {
    const st = new URLSearchParams(path.split('?')[1] ?? '').get('status') ?? 'all'
    const items = invites.filter((i) =>
      st === 'all'
        ? true
        : st === 'revoked'
          ? !!i.revoked_at
          : st === 'used'
            ? !i.revoked_at && i.used_count >= i.max_uses
            : !i.revoked_at && i.used_count < i.max_uses,
    )
    return ok({ items: [...items].sort((a, b) => b.created_at - a.created_at), cursor: null, default_quota: defaultQuota })
  }
  if (path === '/admin/invites' && method === 'POST') {
    const items: Invite[] = Array.from({ length: body.count ?? 1 }, () => ({
      code: genCode(),
      days: body.days,
      quota: body.quota ?? defaultQuota,
      max_uses: body.max_uses ?? 1,
      used_count: 0,
      note: body.note || null,
      created_by_email: state.user!.email,
      created_at: now(),
      expires_at: body.expires_at ?? null,
      revoked_at: null,
      redemptions: [],
      gateway_budget: body.gateway_budget ? Number(body.gateway_budget).toFixed(2) : null,
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
      {
        id: 'p' + u.id,
        kind: 'qq_official',
        display_name: 'QQ 官方机器人',
        app_id: '10238' + u.id.slice(1),
        secret_masked: 'Xk3…9fQa',
        enabled: true,
        status: u.errors ? 'error' : 'connected',
        status_detail: u.errors ? 'AppSecret 已失效，请重新扫码' : undefined,
      },
    ],
    plugins: Array.from({ length: u.plugins }, (_, i) => ({
      ...samplePlugin('astrbot_plugin_demo' + i, '示例插件 ' + (i + 1), 12),
      status: 'running' as const,
    })),
  }
}

function mockSystem(): AdminSystem {
  const t = now()
  const wave = (i: number, base: number, amp: number) => Math.max(0, base + amp * Math.sin(i / 9) + amp * 0.4 * Math.sin(i / 2.3))
  const history = Array.from({ length: 240 }, (_, i) => ({
    ts: t - (239 - i) * 15_000,
    host_cpu: Math.round(wave(i, 18, 9) * 10) / 10,
    container_cpu: Math.round(wave(i, 9, 6) * 10) / 10,
    host_memory_used_bytes: Math.round((1180 + wave(i, 40, 30)) * MB),
    container_memory_bytes: Math.round((402 + wave(i, 20, 14)) * MB),
  }))
  const plug = (
    pid: number,
    user_id: string,
    bot_id: string,
    bot_name: string,
    plugin_id: string,
    label: string,
    pss: number,
    cpu: number,
  ) => ({
    pid,
    ppid: 41,
    role: 'plugin' as const,
    name: 'python3',
    rss_bytes: Math.round(pss * 1.9 * MB),
    pss_bytes: pss * MB,
    uss_bytes: Math.round(pss * 0.8 * MB),
    swap_bytes: 0,
    cpu_percent: cpu,
    threads: 3,
    started_at: t - pid * 60_000,
    plugin: { user_id, bot_id, bot_name, plugin_id, label },
  })
  return {
    generated_at: t,
    console: {
      colo: 'HKG',
      d1: {
        size_bytes: 1_327_104,
        query_ms: 0.61,
        round_trip_ms: 14,
        tables: [
          ['users', 61],
          ['sessions', 74],
          ['bots', 4],
          ['platforms', 5],
          ['plugins', 17],
          ['llm_keys', 6],
          ['invite_codes', 40],
          ['invite_redemptions', 31],
          ['gateway_accounts', 2],
          ['gateway_credits', 3],
          ['email_codes', 3],
          ['captchas', 12],
          ['rate_events', 208],
          ['qr_tasks', 2],
          ['node_tombstones', 0],
          ['nodes', 1],
        ].map(([name, rows]) => ({ name: name as string, rows: rows as number })),
        users_admin: 1,
        users_login_24h: 9,
        sessions_active: 52,
        bots_desired_running: 4,
        bots_needs_push: 0,
        qr_tasks_active: 1,
      },
    },
    users: { u1: 'qunzhu@example.com', u2: 'astr.fan@example.com', u0: 'xiaoyu@example.com' },
    nodes: [
      {
        id: 'node-1',
        base_url: 'https://fish-node-1.niox.lol',
        online: true,
        latency_ms: 187,
        last_seen: t,
        system: {
          node_id: 'node-1',
          version: '0.1.0',
          started_at: t - 3 * 3600_000 - 720_000,
          interval_s: 15,
          sampled_at: t - 6_000,
          host: {
            cpu_count: 2,
            cpu_percent: 21.4,
            load: [0.31, 0.24, 0.19],
            uptime_s: 23 * 86400 + 4 * 3600,
            memory_total_bytes: 3797 * MB,
            memory_available_bytes: 2563 * MB,
            swap_total_bytes: 0,
            swap_free_bytes: 0,
            kernel: '6.8.0-51-generic',
          },
          container: {
            memory_current_bytes: 418 * MB,
            memory_max_bytes: 1600 * MB,
            memory_peak_bytes: 702 * MB,
            memory_anon_bytes: 281 * MB,
            memory_file_bytes: 119 * MB,
            oom_kills: 0,
            cpu_percent: 11.8,
            cpu_limit_cores: null,
            pids_current: 23,
            pids_max: 512,
          },
          processes: [
            plug(88, 'u2', 'b2', 'Astr 搬家', 'pl2', 'meme_manager', 96, 6.2),
            plug(91, 'u1', 'b1', '群聊小鱼', 'pl1', 'daily_quote', 48, 0.4),
            plug(93, 'u0', 'b0', '大肥鱼', 'pl0', 'astrbot_plugin_weather', 41, 0.1),
            {
              pid: 7,
              ppid: 1,
              role: 'node',
              name: 'fish-node',
              rss_bytes: 38 * MB,
              pss_bytes: 36 * MB,
              uss_bytes: 34 * MB,
              swap_bytes: 0,
              cpu_percent: 2.1,
              threads: 9,
              started_at: t - 3 * 3600_000 - 720_000,
            },
            {
              pid: 41,
              ppid: 40,
              role: 'sidecar',
              name: 'python3',
              rss_bytes: 61 * MB,
              pss_bytes: 29 * MB,
              uss_bytes: 18 * MB,
              swap_bytes: 0,
              cpu_percent: 0.1,
              threads: 1,
              started_at: t - 3 * 3600_000,
            },
            {
              pid: 40,
              ppid: 7,
              role: 'sidecar',
              name: 'bwrap',
              rss_bytes: 2 * MB,
              pss_bytes: 1 * MB,
              uss_bytes: 1 * MB,
              swap_bytes: 0,
              cpu_percent: 0,
              threads: 1,
              started_at: t - 3 * 3600_000,
            },
            {
              pid: 1,
              ppid: 0,
              role: 'other',
              name: 'tini',
              rss_bytes: 1 * MB,
              pss_bytes: 0.4 * MB,
              uss_bytes: 0.3 * MB,
              swap_bytes: 0,
              cpu_percent: 0,
              threads: 1,
              started_at: t - 3 * 3600_000 - 730_000,
            },
          ],
          history,
          disk: {
            path: '/data',
            total_bytes: 69 * 1024 * MB,
            free_bytes: 59 * 1024 * MB,
            avail_bytes: 56 * 1024 * MB,
            inodes_total: 4_587_520,
            inodes_free: 4_201_334,
          },
          storage: { node_db_bytes: 2 * MB, conversations_db_bytes: 11 * MB, tenants_bytes: 1232 * MB, uv_cache_bytes: 1840 * MB },
          cgroups_enabled: true,
          tenants: [
            {
              user_id: 'u2',
              memory_current_bytes: 131 * MB,
              memory_max_bytes: 256 * MB,
              memory_peak_bytes: 241 * MB,
              oom_kills: 1,
              procs: 1,
            },
            {
              user_id: 'u1',
              memory_current_bytes: 66 * MB,
              memory_max_bytes: 256 * MB,
              memory_peak_bytes: 90 * MB,
              oom_kills: 0,
              procs: 1,
            },
            {
              user_id: 'u0',
              memory_current_bytes: 58 * MB,
              memory_max_bytes: 128 * MB,
              memory_peak_bytes: 61 * MB,
              oom_kills: 0,
              procs: 1,
            },
          ],
          bots: { total: 4, running: 3, error: 1 },
          platforms: [
            { kind: 'qq_official', total: 3, connected: 2, error: 1 },
            { kind: 'weixin_ilink', total: 2, connected: 1, connecting: 1 },
          ],
          plugins: { total: 17, running: 13, sleeping: 2, error: 1, over_quota: 1 },
          logs: {
            window_s: 3600,
            errors: 7,
            warns: 12,
            bots: [
              {
                bot_id: 'b1',
                bot_name: '群聊小鱼',
                user_id: 'u1',
                errors: 5,
                warns: 3,
                last_error_at: t - 4 * 60_000,
                last_error: 'QQ 网关断开：invalid session，3 秒后重连',
              },
              {
                bot_id: 'b2',
                bot_name: 'Astr 搬家',
                user_id: 'u2',
                errors: 2,
                warns: 9,
                last_error_at: t - 31 * 60_000,
                last_error: '插件 meme_manager 超出内存额度，已停止',
              },
            ],
          },
        },
      },
    ],
  }
}
