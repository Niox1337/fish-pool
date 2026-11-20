import { useState, type FormEvent } from 'react'
import { Link, NavLink, useParams } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, Puzzle } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { keys, useAdminBotLogs, useAdminUser, useAdminUserUsage } from '@/api/hooks'
import type { AdminUser, Bot, Quota } from '@/api/types'
import { Button, ConfirmDialog, ErrorNote, Field, Input, Skeleton, useFieldId } from '@/components/ui'
import { Pupil, statusMood } from '@/components/Pupil'
import { BotStatusPill, PlatformBadge, PluginStatusPill } from '@/pages/console/bot/status'
import { cn } from '@/lib/cn'
import { daysLeft, fmtAgo, fmtDate } from '@/lib/format'
import { UsageSkeleton, UsageView } from '../usage/UsageView'
import { LogList, QuotaInputs, ServiceStatus, quotaFieldError, quotaFields, quotaFromForm, quotaToForm } from './shared'

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
