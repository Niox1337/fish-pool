import { useState, type FormEvent } from 'react'
import { Link } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Copy, CopyCheck, Plus, X } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { keys, useAdminInvites } from '@/api/hooks'
import type { Invite } from '@/api/types'
import { Button, ConfirmDialog, ErrorNote, Field, Input, PageHeader, Pill, Skeleton, useFieldId } from '@/components/ui'
import { cn } from '@/lib/cn'
import { fmtDate, fmtDateTime } from '@/lib/format'
import { copyText } from './copy'

type Filter = 'unused' | 'used' | 'revoked' | 'all'
const filters: { key: Filter; label: string }[] = [
  { key: 'unused', label: '未使用' },
  { key: 'used', label: '已用完' },
  { key: 'revoked', label: '已撤销' },
  { key: 'all', label: '全部' },
]

function endOfDay(value: string) {
  const [y, m, d] = value.split('-').map(Number)
  return new Date(y, m - 1, d, 23, 59, 59).getTime()
}

function todayInput() {
  const d = new Date()
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

type Errors = Partial<Record<'days' | 'count' | 'max_uses', string>>

function CreateForm({ onCreated }: { onCreated: (items: Invite[]) => void }) {
  const qc = useQueryClient()
  const [days, setDays] = useState('30')
  const [count, setCount] = useState('1')
  const [maxUses, setMaxUses] = useState('1')
  const [note, setNote] = useState('')
  const [expires, setExpires] = useState('')
  const [busy, setBusy] = useState(false)
  const [errors, setErrors] = useState<Errors>({})
  const ids = { days: useFieldId('inv-days'), count: useFieldId('inv-count'), uses: useFieldId('inv-uses'), note: useFieldId('inv-note'), exp: useFieldId('inv-exp') }

  async function submit(e: FormEvent) {
    e.preventDefault()
    const d = Number(days)
    const c = Number(count)
    const u = Number(maxUses)
    const next: Errors = {}
    if (!Number.isInteger(d) || d < 1 || d > 3650) next.days = '服务天数填 1 到 3650'
    if (!Number.isInteger(c) || c < 1 || c > 50) next.count = '一次生成 1 到 50 个'
    if (!Number.isInteger(u) || u < 1 || u > 1000) next.max_uses = '每个码可用 1 到 1000 次'
    setErrors(next)
    if (Object.keys(next).length) return
    setBusy(true)
    try {
      const r = await api.adminCreateInvites({ days: d, count: c, max_uses: u, note: note.trim() || undefined, expires_at: expires ? endOfDay(expires) : null })
      qc.invalidateQueries({ queryKey: [...keys.admin, 'invites'] })
      qc.invalidateQueries({ queryKey: [...keys.admin, 'overview'] })
      onCreated(r.items)
      toast.success(`生成了 ${r.items.length} 个邀请码`)
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '没能生成，稍后再试')
    } finally {
      setBusy(false)
    }
  }

  return (
    <form onSubmit={submit} noValidate className="grid gap-4 rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <h2 className="font-display text-lg font-black text-ink">生成邀请码</h2>
      <Field label="服务天数" htmlFor={ids.days} error={errors.days} hint="兑换后给对方的服务期限加上这么多天。">
        <div className="flex flex-wrap items-center gap-2">
          <Input
            id={ids.days}
            type="number"
            inputMode="numeric"
            min={1}
            max={3650}
            required
            value={days}
            onChange={(e) => setDays(e.target.value)}
            aria-invalid={!!errors.days}
            className="w-28 tabular-nums"
          />
          <div className="flex flex-wrap gap-1.5" role="group" aria-label="常用天数">
            {[7, 30, 90, 365].map((d) => (
              <button
                key={d}
                type="button"
                onClick={() => setDays(String(d))}
                aria-pressed={days === String(d)}
                className={cn(
                  'cursor-pointer rounded-full border px-3 py-1 text-[13px] font-semibold tabular-nums transition-colors duration-150',
                  days === String(d) ? 'border-cobalt bg-cobalt text-white' : 'border-paper-3 bg-white text-ink-2 hover:border-ink-3/50 hover:text-ink',
                )}
              >
                {d} 天
              </button>
            ))}
          </div>
        </div>
      </Field>
      <div className="grid grid-cols-2 gap-4">
        <Field label="生成个数" htmlFor={ids.count} error={errors.count}>
          <Input id={ids.count} type="number" inputMode="numeric" min={1} max={50} value={count} onChange={(e) => setCount(e.target.value)} aria-invalid={!!errors.count} className="tabular-nums" />
        </Field>
        <Field label="每个码可用次数" htmlFor={ids.uses} error={errors.max_uses}>
          <Input id={ids.uses} type="number" inputMode="numeric" min={1} max={1000} value={maxUses} onChange={(e) => setMaxUses(e.target.value)} aria-invalid={!!errors.max_uses} className="tabular-nums" />
        </Field>
      </div>
      <Field label="备注" htmlFor={ids.note} hint="只有管理员看得到，比如发给谁、哪个群。">
        <Input id={ids.note} value={note} onChange={(e) => setNote(e.target.value)} maxLength={80} placeholder="群友试用" />
      </Field>
      <Field label="兑换截止日" htmlFor={ids.exp} hint="可选。过了这天码就不能兑换，已经兑换的服务不受影响。">
        <div className="flex flex-wrap items-center gap-2">
          <Input id={ids.exp} type="date" min={todayInput()} value={expires} onChange={(e) => setExpires(e.target.value)} className="w-44 tabular-nums" />
          {expires && (
            <Button size="sm" variant="ghost" onClick={() => setExpires('')}>
              不设截止
            </Button>
          )}
        </div>
      </Field>
      <div>
        <Button type="submit" variant="primary" loading={busy}>
          <Plus aria-hidden />
          生成
        </Button>
      </div>
    </form>
  )
}

function Created({ items, onClose }: { items: Invite[]; onClose: () => void }) {
  const [copied, setCopied] = useState<string | null>(null)
  async function copy(code: string) {
    await copyText(code, '邀请码')
    setCopied(code)
  }
  return (
    <section aria-labelledby="inv-created" className="rounded-2xl border border-star-2/70 bg-cream p-5 sm:p-6" role="status">
      <div className="flex flex-wrap items-center gap-2">
        <h2 id="inv-created" className="flex-1 font-display text-lg font-black text-ink">
          刚生成的 {items.length} 个
        </h2>
        <Button size="sm" variant="primary" onClick={() => copyText(items.map((i) => i.code).join('\n'), `${items.length} 个邀请码`)}>
          <Copy aria-hidden />
          全部复制
        </Button>
        <Button size="sm" variant="ghost" onClick={onClose} aria-label="收起刚生成的邀请码">
          <X aria-hidden />
        </Button>
      </div>
      <p className="mt-1 text-[13px] text-ink-3 tabular-nums">
        每个 {items[0]?.days} 天，可用 {items[0]?.max_uses} 次{items[0]?.expires_at ? `，${fmtDate(items[0].expires_at)} 前兑换` : ''}
      </p>
      <ul className="mt-3 grid gap-2 sm:grid-cols-2">
        {items.map((i) => (
          <li key={i.code} className="flex items-center gap-2 rounded-xl border border-paper-3 bg-white py-1.5 pr-1.5 pl-3">
            <code className="flex-1 font-mono text-[15px] font-semibold tracking-wider text-ink select-all">{i.code}</code>
            <Button size="sm" variant="ghost" onClick={() => copy(i.code)} aria-label={`复制 ${i.code}`}>
              {copied === i.code ? <CopyCheck aria-hidden className="text-ok" /> : <Copy aria-hidden />}
              {copied === i.code ? '已复制' : '复制'}
            </Button>
          </li>
        ))}
      </ul>
    </section>
  )
}

function inviteState(i: Invite): { label: string; tone: 'ok' | 'idle' | 'bad' | 'warn' } {
  if (i.revoked_at) return { label: '已撤销', tone: 'bad' }
  if (i.used_count >= i.max_uses) return { label: '已用完', tone: 'idle' }
  if (i.expires_at && i.expires_at < Date.now()) return { label: '已过期', tone: 'warn' }
  return { label: '可兑换', tone: 'ok' }
}

function InviteRow({ inv }: { inv: Invite }) {
  const qc = useQueryClient()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const st = inviteState(inv)

  async function revoke() {
    setBusy(true)
    try {
      await api.adminRevokeInvite(inv.code)
      qc.invalidateQueries({ queryKey: [...keys.admin, 'invites'] })
      qc.invalidateQueries({ queryKey: [...keys.admin, 'overview'] })
      toast.success(`${inv.code} 已撤销`)
      setOpen(false)
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '没能撤销，稍后再试')
    } finally {
      setBusy(false)
    }
  }

  return (
    <li className="px-4 py-3.5 sm:px-5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <code className={cn('font-mono text-[15px] font-semibold tracking-wider text-ink select-all', inv.revoked_at && 'text-ink-3 line-through')}>{inv.code}</code>
        <Pill tone={st.tone}>{st.label}</Pill>
        <span className="text-sm text-ink tabular-nums">{inv.days} 天</span>
        <span className="text-sm text-ink-2 tabular-nums" title="已用次数 / 可用次数">
          已用 {inv.used_count}/{inv.max_uses}
        </span>
        <span className="ml-auto flex gap-1">
          <Button size="sm" variant="ghost" onClick={() => copyText(inv.code, '邀请码')} aria-label={`复制 ${inv.code}`}>
            <Copy aria-hidden />
            复制
          </Button>
          {!inv.revoked_at && (
            <Button size="sm" variant="ghost" className="text-bad hover:bg-bad-soft hover:text-bad" onClick={() => setOpen(true)}>
              撤销
            </Button>
          )}
        </span>
      </div>
      {inv.note && <p className="mt-1 text-sm text-ink">{inv.note}</p>}
      <p className="mt-1 flex flex-wrap gap-x-4 gap-y-0.5 text-[13px] text-ink-3 tabular-nums">
        <span>{inv.created_by_email} 于 {fmtDate(inv.created_at)} 生成</span>
        <span>{inv.expires_at ? `${fmtDate(inv.expires_at)} 前可兑换` : '不限兑换日期'}</span>
        {inv.revoked_at && <span className="text-bad">{fmtDate(inv.revoked_at)} 撤销</span>}
      </p>
      {inv.redemptions.length > 0 && (
        <ul className="mt-2 grid gap-0.5 rounded-lg bg-paper-2/60 px-3 py-1.5 text-[13px]">
          {inv.redemptions.map((r) => (
            <li key={`${r.user_id}-${r.redeemed_at}`} className="flex flex-wrap gap-x-3">
              <Link to={`/app/admin/users/${r.user_id}`} className="break-all font-semibold text-sky">
                {r.user_email}
              </Link>
              <span className="text-ink-3 tabular-nums">
                {fmtDateTime(r.redeemed_at)} 兑换，加了 {r.days} 天
              </span>
            </li>
          ))}
        </ul>
      )}
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={`撤销 ${inv.code}？`}
        body={
          inv.used_count > 0
            ? `撤销后这个码不能再兑换。已经兑换的 ${inv.used_count} 次服务时间保留，不会收回。`
            : '撤销后这个码不能再兑换。如果已经发给别人，记得告诉对方。'
        }
        confirmLabel="撤销邀请码"
        onConfirm={revoke}
        loading={busy}
      />
    </li>
  )
}

export default function AdminInvites() {
  const [filter, setFilter] = useState<Filter>('unused')
  const [created, setCreated] = useState<Invite[] | null>(null)
  const list = useAdminInvites(filter)
  const items = list.data?.items ?? []

  return (
    <>
      <PageHeader title="邀请码" description="现在只能凭邀请码入学。每个码带一段服务天数，注册时或在账号页兑换都会加到对方的服务期限上。" />
      <div className="grid gap-6 lg:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)] lg:items-start">
        <div className="grid gap-6">
          <CreateForm onCreated={setCreated} />
          {created && <Created items={created} onClose={() => setCreated(null)} />}
        </div>

        <section aria-labelledby="inv-list">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
            <h2 id="inv-list" className="font-display text-lg font-black text-ink">
              所有邀请码
            </h2>
            <div role="radiogroup" aria-label="按状态筛选" className="flex rounded-xl border border-paper-3 bg-paper-2 p-0.5">
              {filters.map((f) => (
                <button
                  key={f.key}
                  type="button"
                  role="radio"
                  aria-checked={filter === f.key}
                  onClick={() => setFilter(f.key)}
                  className={cn(
                    'cursor-pointer rounded-[10px] px-3 py-1 text-[13px] font-semibold transition-colors duration-150',
                    filter === f.key ? 'bg-cream text-ink shadow-[0_1px_2px_rgb(27_33_99/0.12)]' : 'text-ink-3 hover:text-ink',
                  )}
                >
                  {f.label}
                </button>
              ))}
            </div>
          </div>
          {list.isLoading ? (
            <div className="grid gap-2">
              <Skeleton className="h-20" />
              <Skeleton className="h-20" />
              <Skeleton className="h-20" />
            </div>
          ) : list.error ? (
            <ErrorNote error={list.error} onRetry={() => list.refetch()} />
          ) : items.length === 0 ? (
            <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-[15px] text-ink-2">
              {filter === 'unused'
                ? '没有能兑换的邀请码了。用左边的表单生成一批，复制发给想入学的人。'
                : filter === 'used'
                  ? '还没有用完的邀请码。'
                  : filter === 'revoked'
                    ? '没有撤销过邀请码。'
                    : '还没有生成过邀请码。'}
            </p>
          ) : (
            <ul className={cn('divide-y divide-paper-3 rounded-2xl border border-paper-3 bg-cream transition-opacity duration-150', list.isFetching && 'opacity-70')}>
              {items.map((inv) => (
                <InviteRow key={inv.code} inv={inv} />
              ))}
            </ul>
          )}
        </section>
      </div>
    </>
  )
}
