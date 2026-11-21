import { useState, type FormEvent } from 'react'
import { Link, NavLink, useParams } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, Puzzle } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { keys, useAdminBotLogs, useAdminCredits, useAdminUser, useAdminUserUsage } from '@/api/hooks'
import type { AdminCredit, AdminCredits, AdminUser, Bot, Quota } from '@/api/types'
import { Button, ConfirmDialog, ErrorNote, Field, Input, Pill, Skeleton } from '@/components/ui'
import { useFieldId } from '@/lib/fieldId'
import { Pupil } from '@/components/Pupil'
import { statusMood } from '@/components/mood'
import { BotStatusPill, PlatformBadge, PluginStatusPill } from '@/pages/console/bot/status'
import { cn } from '@/lib/cn'
import { daysLeft, fmtAgo, fmtDate } from '@/lib/format'
import { UsageSkeleton, UsageView } from '../usage/UsageView'
import { LogList, QuotaInputs, ServiceStatus } from './shared'
import { quotaFieldError, quotaFields, quotaFromForm, quotaToForm } from './quota'

type Detail = { user: AdminUser; bots: Bot[] }

function usePatchUser(userId: string) {
  const qc = useQueryClient()
  return async (body: Parameters<typeof api.adminPatchUser>[1], done: (u: AdminUser) => string) => {
    try {
      const u = await api.adminPatchUser(userId, body)
      qc.setQueryData<Detail>([...keys.admin, 'user', userId], (old) => (old ? { ...old, user: u } : old))
      qc.invalidateQueries({ queryKey: keys.admin })
      qc.invalidateQueries({ queryKey: keys.me })
      toast.success(done(u))
      return true
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '没能保存，稍后再试')
      return false
    }
  }
}

function toDateInput(ms: number | null) {
  if (!ms) return ''
  const d = new Date(ms)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function endOfDay(value: string) {
  const [y, m, d] = value.split('-').map(Number)
  return new Date(y, m - 1, d, 23, 59, 59).getTime()
}

function ServicePanel({ user }: { user: AdminUser }) {
  const patch = usePatchUser(user.id)
  const [busy, setBusy] = useState<string | null>(null)
  const [custom, setCustom] = useState('')
  const [date, setDate] = useState(toDateInput(user.service_until))
  const [clearOpen, setClearOpen] = useState(false)
  const customId = useFieldId('svc-days')
  const dateId = useFieldId('svc-date')
  const left = daysLeft(user.service_until)
  const [syncedUntil, setSyncedUntil] = useState(user.service_until)
  if (syncedUntil !== user.service_until) {
    setSyncedUntil(user.service_until)
    setDate(toDateInput(user.service_until))
  }

  async function run(tag: string, body: Parameters<typeof api.adminPatchUser>[1], msg: (u: AdminUser) => string) {
    setBusy(tag)
    const ok = await patch(body, msg)
    setBusy(null)
    return ok
  }

  const extended = (u: AdminUser) => `服务已延长到 ${fmtDate(u.service_until)}`

  function submitCustom(e: FormEvent) {
    e.preventDefault()
    const d = Number(custom)
    if (!Number.isInteger(d) || d < 1 || d > 3650) {
      toast.error('天数填 1 到 3650 之间的整数')
      return
    }
    run('custom', { extend_days: d }, extended).then((ok) => ok && setCustom(''))
  }

  function submitDate(e: FormEvent) {
    e.preventDefault()
    if (!date) return
    run('date', { service_until: endOfDay(date) }, (u) => `服务到期日改为 ${fmtDate(u.service_until)}`)
  }

  return (
    <section aria-labelledby="ud-service" className="rounded-2xl border border-paper-3 bg-cream p-5">
      <h2 id="ud-service" className="font-display text-lg font-black text-ink">
        服务期限
      </h2>
      <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[15px]">
        <ServiceStatus user={user} compact />
        {user.role !== 'admin' && user.service_until != null && (
          <span className="text-ink-2 tabular-nums">
            {left != null && left > 0 ? `到 ${fmtDate(user.service_until)}，还剩 ${left} 天` : `${fmtDate(user.service_until)} 到期`}
          </span>
        )}
      </div>
      {user.role === 'admin' && <p className="mt-2 text-[13px] text-ink-3">到期日仅对普通用户生效。</p>}

      <div className="mt-5 grid gap-5">
        <div>
          <p className="mb-2 text-sm font-semibold text-ink">延长</p>
          <div className="flex flex-wrap gap-2">
            {[7, 30, 90].map((d) => (
              <Button key={d} size="sm" loading={busy === `+${d}`} disabled={!!busy} onClick={() => run(`+${d}`, { extend_days: d }, extended)}>
                <span className="tabular-nums">+{d} 天</span>
              </Button>
            ))}
          </div>
          <form onSubmit={submitCustom} className="mt-3 flex flex-wrap items-center gap-2">
            <label htmlFor={customId} className="sr-only">
              自定义延长天数
            </label>
            <Input
              id={customId}
              type="number"
              inputMode="numeric"
              min={1}
              max={3650}
              value={custom}
              onChange={(e) => setCustom(e.target.value)}
              placeholder="自定义天数"
              className="h-8 w-32 rounded-lg text-sm tabular-nums"
            />
            <Button type="submit" size="sm" loading={busy === 'custom'} disabled={!!busy || !custom}>
              延长
            </Button>
          </form>
          <p className="mt-1.5 text-[13px] text-ink-3">已到期的账号从今天续期。</p>
        </div>

        <form onSubmit={submitDate} className="border-t border-paper-3 pt-4">
          <Field label="设定到期日" htmlFor={dateId} hint="当天结束时到期。">
            <div className="flex flex-wrap items-center gap-2">
              <Input id={dateId} type="date" value={date} onChange={(e) => setDate(e.target.value)} className="h-8 w-44 rounded-lg text-sm tabular-nums" />
              <Button type="submit" size="sm" loading={busy === 'date'} disabled={!!busy || !date || date === toDateInput(user.service_until)}>
                设定
              </Button>
              {user.service_until != null && (
                <Button size="sm" variant="danger" disabled={!!busy} onClick={() => setClearOpen(true)}>
                  清除服务
                </Button>
              )}
            </div>
          </Field>
        </form>
      </div>
      <ConfirmDialog
        open={clearOpen}
        onOpenChange={setClearOpen}
        title="清除这位用户的服务？"
        body={`${user.email} 会变成未开通状态，小鱼会在几分钟内停下，数据保留。之后兑换邀请码或在这里延长就能恢复。`}
        confirmLabel="清除服务"
        loading={busy === 'clear'}
        onConfirm={() => run('clear', { service_until: null }, () => '服务已清除').then(() => setClearOpen(false))}
      />
    </section>
  )
}

function QuotaEditor({ user }: { user: AdminUser }) {
  const patch = usePatchUser(user.id)
  const [form, setForm] = useState(() => quotaToForm(user.quota))
  const [busy, setBusy] = useState(false)
  const quotaSig = JSON.stringify(user.quota)
  const [syncedQuota, setSyncedQuota] = useState(quotaSig)
  if (syncedQuota !== quotaSig) {
    setSyncedQuota(quotaSig)
    setForm(quotaToForm(user.quota))
  }

  const nextQuota = quotaFromForm(form)
  const changed = quotaFields.filter((f) => nextQuota[f.key] !== user.quota[f.key])
  const valid = quotaFields.every((f) => !quotaFieldError(form[f.key], f))

  async function save(e: FormEvent) {
    e.preventDefault()
    if (!valid || changed.length === 0) return
    setBusy(true)
    const quota = Object.fromEntries(changed.map((f) => [f.key, nextQuota[f.key]])) as Partial<Quota>
    await patch({ quota }, () => '额度已保存，正在同步到节点')
    setBusy(false)
  }

  return (
    <form onSubmit={save} noValidate aria-labelledby="ud-quota" className="rounded-2xl border border-paper-3 bg-cream p-5">
      <h2 id="ud-quota" className="font-display text-lg font-black text-ink">
        额度
      </h2>
      <div className="mt-4">
        <QuotaInputs form={form} onChange={(key, value) => setForm((prev) => ({ ...prev, [key]: value }))} />
      </div>
      <div className="mt-4 flex flex-wrap items-center gap-3">
        <Button type="submit" variant="primary" size="sm" loading={busy} disabled={!valid || changed.length === 0}>
          保存额度
        </Button>
        {changed.length > 0 && (
          <Button size="sm" variant="ghost" onClick={() => setForm(quotaToForm(user.quota))}>
            还原
          </Button>
        )}
        <span className="text-[13px] text-ink-3">保存后会推送到这位用户小鱼所在的节点。</span>
      </div>
    </form>
  )
}

const MAX_CREDIT_YUAN = 1000

/** Error for a credit amount in yuan, null when valid. */
function creditAmountError(v: string): string | null {
  const t = v.trim()
  if (!/^\d{1,4}(\.\d{1,2})?$/.test(t) || Number(t) <= 0 || Number(t) > MAX_CREDIT_YUAN) return `填 0.01 到 ${MAX_CREDIT_YUAN} 元，最多两位小数`
  return null
}

function useCreditAction(userId: string) {
  const qc = useQueryClient()
  return async (run: () => Promise<AdminCredits>, done: string) => {
    try {
      const data = await run()
      qc.setQueryData([...keys.admin, 'credits', userId], data)
      toast.success(done)
      return true
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '没能保存，稍后再试')
      return false
    }
  }
}

const creditState: Record<AdminCredit['state'], { tone: 'ok' | 'warn' | 'bad'; label: string }> = {
  granted: { tone: 'ok', label: '已到账' },
  pending: { tone: 'warn', label: '发放中' },
  failed: { tone: 'bad', label: '发放失败' },
}

function CreditRow({ userId, c }: { userId: string; c: AdminCredit }) {
  const act = useCreditAction(userId)
  const [editing, setEditing] = useState(false)
  const [amount, setAmount] = useState(c.amount)
  const [date, setDate] = useState(toDateInput(c.expires_at))
  const [busy, setBusy] = useState(false)
  const amountId = useFieldId('credit-amount')
  const dateId = useFieldId('credit-date')
  const amountError = creditAmountError(amount)
  const state = creditState[c.state]

  function open() {
    setAmount(c.amount)
    setDate(toDateInput(c.expires_at))
    setEditing(true)
  }

  async function save(e: FormEvent) {
    e.preventDefault()
    if (amountError) return
    const body: { amount?: string; expires_at?: number | null } = {}
    if (Number(amount) !== Number(c.amount)) body.amount = amount.trim()
    const nextExpiry = date ? endOfDay(date) : null
    if (date !== toDateInput(c.expires_at)) body.expires_at = nextExpiry
    if (Object.keys(body).length === 0) {
      setEditing(false)
      return
    }
    setBusy(true)
    const ok = await act(() => api.adminPatchCredit(userId, c.id, body), '额度已修改')
    setBusy(false)
    if (ok) setEditing(false)
  }

  return (
    <li className="py-3">
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <div className="min-w-0">
          <p className="font-semibold text-ink tabular-nums">
            {c.remaining != null ? `剩 ${c.remaining} 元` : `${c.amount} 元`}
            {c.remaining != null && <span className="ml-2 text-sm font-normal text-ink-3">共 {c.amount} 元</span>}
          </p>
          <p className="mt-0.5 text-[13px] wrap-anywhere text-ink-3">
            {c.source === 'invite' ? '邀请码赠送' : `管理员添加${c.created_by_email ? `（${c.created_by_email}）` : ''}`}
            {c.note && ` · ${c.note}`}
          </p>
        </div>
        <div className="flex items-center gap-2 text-sm">
          <span className={cn('tabular-nums', c.expired ? 'text-bad' : 'text-ink-2')}>
            {c.expires_at == null ? '不过期' : c.expired ? `${fmtDate(c.expires_at)} 已过期` : `${fmtDate(c.expires_at)} 过期`}
          </span>
          <Pill tone={state.tone}>{state.label}</Pill>
        </div>
      </div>
      {c.error && c.state !== 'granted' && <p className="mt-1 text-[13px] wrap-anywhere text-bad">{c.error}</p>}
      {editing ? (
        <form onSubmit={save} noValidate className="mt-3 grid gap-3 rounded-xl bg-paper-2/60 p-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
          <Field label="总额度（元）" htmlFor={amountId} error={amount && amountError ? amountError : undefined}>
            <Input id={amountId} inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} className="h-8 rounded-lg text-sm tabular-nums" />
          </Field>
          <Field label="过期日" htmlFor={dateId} hint="留空则不过期">
            <Input id={dateId} type="date" value={date} onChange={(e) => setDate(e.target.value)} className="h-8 rounded-lg text-sm tabular-nums" />
          </Field>
          <div className="flex gap-2">
            <Button type="submit" size="sm" variant="primary" loading={busy} disabled={!!amountError}>
              保存
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setEditing(false)}>
              取消
            </Button>
          </div>
        </form>
      ) : (
        <Button size="sm" variant="ghost" className="mt-1.5" onClick={open}>
          修改
        </Button>
      )}
    </li>
  )
}

function CreditPanel({ user }: { user: AdminUser }) {
  const q = useAdminCredits(user.id)
  const act = useCreditAction(user.id)
  const [amount, setAmount] = useState('')
  const [date, setDate] = useState('')
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const amountId = useFieldId('credit-new-amount')
  const dateId = useFieldId('credit-new-date')
  const noteId = useFieldId('credit-new-note')
  const amountError = amount ? creditAmountError(amount) : null
  const data = q.data

  async function add(e: FormEvent) {
    e.preventDefault()
    if (!amount || amountError) return
    setBusy(true)
    const ok = await act(
      () => api.adminAddCredit(user.id, { amount: amount.trim(), expires_at: date ? endOfDay(date) : null, note: note.trim() || undefined }),
      `已添加 ${amount.trim()} 元活动额度`,
    )
    setBusy(false)
    if (ok) {
      setAmount('')
      setDate('')
      setNote('')
    }
  }

  return (
    <section aria-labelledby="ud-credits" className="rounded-2xl border border-paper-3 bg-cream p-5">
      <h2 id="ud-credits" className="font-display text-lg font-black text-ink">
        活动额度
      </h2>
      <p className="mt-1 text-[13px] text-ink-3">每笔额度单独过期，用户用的时候先扣最早过期的那笔。</p>
      {q.error ? (
        <div className="mt-3">
          <ErrorNote error={q.error} onRetry={() => q.refetch()} />
        </div>
      ) : !data ? (
        <Skeleton className="mt-3 h-16" />
      ) : (
        <>
          {data.remote_error && <p className="mt-2 text-[13px] text-bad">查不到剩余额度：{data.remote_error}</p>}
          {data.items.length === 0 ? (
            <p className="mt-3 text-[15px] text-ink-2">还没有活动额度。</p>
          ) : (
            <ul className="mt-2 divide-y divide-paper-3">
              {data.items.map((c) => (
                <CreditRow key={c.id} userId={user.id} c={c} />
              ))}
            </ul>
          )}
          {data.configured ? (
            <form onSubmit={add} noValidate className="mt-4 grid gap-3 border-t border-paper-3 pt-4 sm:grid-cols-2">
              <Field label="添加额度（元）" htmlFor={amountId} error={amountError ?? undefined}>
                <Input id={amountId} inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="例如 5" className="h-8 rounded-lg text-sm tabular-nums" />
              </Field>
              <Field label="过期日" htmlFor={dateId} hint="留空则不过期">
                <Input id={dateId} type="date" value={date} onChange={(e) => setDate(e.target.value)} className="h-8 rounded-lg text-sm tabular-nums" />
              </Field>
              <Field label="备注（可选）" htmlFor={noteId}>
                <Input id={noteId} value={note} maxLength={200} onChange={(e) => setNote(e.target.value)} className="h-8 rounded-lg text-sm" />
              </Field>
              <div className="flex items-end">
                <Button type="submit" size="sm" variant="primary" loading={busy} disabled={!amount || !!amountError}>
                  添加额度
                </Button>
              </div>
            </form>
          ) : (
            <p className="mt-3 text-[13px] text-ink-3">活动额度服务未配置。</p>
          )}
        </>
      )}
    </section>
  )
}

function BotCard({ bot }: { bot: Bot }) {
  return (
    <li className="rounded-2xl border border-paper-3 bg-cream p-4">
      <div className="flex items-start gap-3">
        <Pupil mood={statusMood[bot.status]} size={48} className="rounded-xl bg-paper-2" />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="truncate font-display font-black text-ink">{bot.name}</h3>
            <BotStatusPill status={bot.status} />
          </div>
          {bot.status_detail && bot.status !== 'running' && <p className="mt-0.5 text-[13px] text-ink-2">{bot.status_detail}</p>}
          <p className="mt-0.5 text-[13px] text-ink-3 tabular-nums">
            {fmtDate(bot.created_at)} 创建，最近更新 {fmtAgo(bot.updated_at)}
          </p>
          <div className="mt-2 flex flex-wrap gap-1.5">
            {bot.platforms.length === 0 ? (
              <span className="text-[13px] text-ink-3">没有接入平台</span>
            ) : (
              bot.platforms.map((p) => <PlatformBadge key={p.id} platform={p} />)
            )}
          </div>
          {bot.platforms.some((p) => p.status === 'error' && p.status_detail) && (
            <ul className="mt-1 text-[13px] text-bad">
              {bot.platforms
                .filter((p) => p.status === 'error' && p.status_detail)
                .map((p) => (
                  <li key={p.id}>
                    {p.display_name}：{p.status_detail}
                  </li>
                ))}
            </ul>
          )}
        </div>
      </div>
      {bot.plugins.length > 0 && (
        <ul className="mt-3 divide-y divide-paper-3/70 border-t border-paper-3 text-sm">
          {bot.plugins.map((p) => (
            <li key={p.id} className="flex flex-wrap items-center gap-x-3 gap-y-0.5 py-2">
              <Puzzle className="size-3.5 shrink-0 text-ink-3" aria-hidden />
              <span className="min-w-0 truncate font-semibold text-ink">{p.display_name}</span>
              <span className="font-mono text-[12px] text-ink-3">{p.version}</span>
              <span className="ml-auto">
                <PluginStatusPill status={p.status} />
              </span>
              {p.status_detail && p.status !== 'running' && <p className="basis-full pl-6 text-[13px] text-ink-2">{p.status_detail}</p>}
            </li>
          ))}
        </ul>
      )}
    </li>
  )
}

function Overview({ data }: { data: Detail }) {
  return (
    <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)] lg:items-start">
      <div className="grid gap-6">
        <ServicePanel user={data.user} />
        <QuotaEditor user={data.user} />
        <CreditPanel user={data.user} />
      </div>
      <section aria-labelledby="ud-bots">
        <h2 id="ud-bots" className="mb-3 font-display text-lg font-black text-ink">
          小鱼
          <span className="ml-2 text-sm font-semibold text-ink-3 tabular-nums">{data.bots.length} 条</span>
        </h2>
        {data.bots.length === 0 ? (
          <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-[15px] text-ink-2">这位用户还没有收小鱼。</p>
        ) : (
          <ul className="grid gap-3">
            {data.bots.map((b) => (
              <BotCard key={b.id} bot={b} />
            ))}
          </ul>
        )}
      </section>
    </div>
  )
}

function UsageSection({ userId, data }: { userId: string; data: Detail }) {
  const usage = useAdminUserUsage(userId)
  if (usage.isLoading) return <UsageSkeleton />
  if (usage.error) return <ErrorNote error={usage.error} onRetry={() => usage.refetch()} />
  if (!usage.data) return null
  return <UsageView usage={usage.data} bots={data.bots} quota={data.user.quota} />
}

function LogsSection({ userId, bots }: { userId: string; bots: Bot[] }) {
  const [picked, setPicked] = useState<string | null>(bots[0]?.id ?? null)
  const botId = bots.some((b) => b.id === picked) ? picked : (bots[0]?.id ?? null)
  const logs = useAdminBotLogs(userId, botId)

  if (bots.length === 0) {
    return <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-[15px] text-ink-2">该用户暂无小鱼。</p>
  }
  return (
    <div className="grid gap-4">
      <div role="radiogroup" aria-label="选择小鱼" className="flex flex-wrap gap-2">
        {bots.map((b) => (
          <button
            key={b.id}
            type="button"
            role="radio"
            aria-checked={b.id === botId}
            onClick={() => setPicked(b.id)}
            className={cn(
              'inline-flex cursor-pointer items-center gap-2 rounded-full border px-3 py-1.5 text-sm font-semibold transition-colors duration-150',
              b.id === botId ? 'border-cobalt bg-cobalt text-white' : 'border-paper-3 bg-cream text-ink-2 hover:border-ink-3/50 hover:text-ink',
            )}
          >
            {b.name}
            <span className={cn('size-2 rounded-full', b.status === 'running' ? 'bg-ok' : b.status === 'error' ? 'bg-bad' : b.status === 'starting' ? 'bg-warn' : 'bg-ink-3')} aria-hidden />
          </button>
        ))}
      </div>
      {logs.isLoading ? (
        <Skeleton className="h-64" />
      ) : logs.error ? (
        <ErrorNote error={logs.error} onRetry={() => logs.refetch()} />
      ) : (logs.data?.items.length ?? 0) === 0 ? (
        <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-[15px] text-ink-2">这条小鱼暂无日志。</p>
      ) : (
        <>
          <LogList items={logs.data!.items} />
          <p className="text-[13px] text-ink-3">每 4 秒自动刷新，只读。</p>
        </>
      )}
    </div>
  )
}

const sections = [
  { key: '', label: '概览' },
  { key: 'usage', label: '用量' },
  { key: 'logs', label: '日志' },
] as const

export default function AdminUserDetail() {
  const { userId = '', section = '' } = useParams()
  const q = useAdminUser(userId)
  const base = `/app/admin/users/${userId}`
  const current = sections.some((s) => s.key === section) ? section : ''

  return (
    <>
      <Link to="/app/admin/users" className="mb-4 inline-flex items-center gap-1.5 text-sm font-semibold text-ink-2 hover:text-ink">
        <ArrowLeft className="size-4" aria-hidden />
        全部用户
      </Link>
      {q.isLoading ? (
        <div className="grid gap-4">
          <Skeleton className="h-9 w-72 max-w-full" />
          <Skeleton className="h-10 w-64" />
          <Skeleton className="h-64" />
        </div>
      ) : q.error ? (
        <ErrorNote error={q.error} onRetry={() => q.refetch()} />
      ) : q.data ? (
        <>
          <header className="mb-5">
            <h1 className="font-display text-2xl font-black break-all text-ink sm:text-[28px]">{q.data.user.email}</h1>
            <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-ink-2">
              <span className={cn('font-semibold', q.data.user.role === 'admin' ? 'text-sky' : 'text-ink')}>{q.data.user.role === 'admin' ? '管理员' : '普通用户'}</span>
              <span className="tabular-nums">{fmtDate(q.data.user.created_at)} 注册</span>
              <span className="tabular-nums">{q.data.user.last_login_at ? `最近登录 ${fmtAgo(q.data.user.last_login_at)}` : '从未登录'}</span>
            </p>
          </header>
          <nav aria-label="用户详情" className="mb-6 flex gap-1 border-b border-paper-3">
            {sections.map((s) => (
              <NavLink
                key={s.key}
                to={s.key ? `${base}/${s.key}` : base}
                end
                className={cn(
                  '-mb-px border-b-2 px-3 py-2 text-[15px] font-semibold transition-colors duration-150',
                  current === s.key ? 'border-cobalt text-ink' : 'border-transparent text-ink-3 hover:text-ink',
                )}
                aria-current={current === s.key ? 'page' : undefined}
              >
                {s.label}
              </NavLink>
            ))}
          </nav>
          {current === '' && <Overview data={q.data} />}
          {current === 'usage' && <UsageSection userId={userId} data={q.data} />}
          {current === 'logs' && <LogsSection userId={userId} bots={q.data.bots} />}
        </>
      ) : null}
    </>
  )
}
