import { useEffect, useRef, useState, type FormEvent } from 'react'
import { Link, useLocation, useNavigate } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Coins, KeyRound, LogOut, RefreshCw, Ticket } from 'lucide-react'
import { toast } from 'sonner'
import { api, ApiError } from '@/api/client'
import { keys, useBots, useGateway, useMe } from '@/api/hooks'
import type { User } from '@/api/types'
import { Button, ConfirmDialog, Field, Input, PageHeader, Pill, Skeleton, Stat } from '@/components/ui'
import { daysLeft, fmtDate, fmtMb } from '@/lib/format'

function formatInvite(value: string): string {
  let raw = value.toUpperCase().replace(/[^0-9A-Z]/g, '')
  if (raw.length < 4 && 'FISH'.startsWith(raw)) return raw
  if (raw.startsWith('FISH')) raw = raw.slice(4)
  raw = raw.slice(0, 8)
  if (!raw) return 'FISH'
  return `FISH-${raw.slice(0, 4)}${raw.length > 4 ? `-${raw.slice(4)}` : ''}`
}

const inviteErrors: Record<string, string> = {
  invite_invalid: '这个邀请码不对或已经作废了，核对一下字母和数字。',
  invite_used: '这个邀请码已经用完了，向发码的人要一个新的。',
  invite_expired: '这个邀请码过了兑换期限，向发码的人要一个新的。',
}

function ServiceLine({ me }: { me: User }) {
  if (me.role === 'admin') {
    return (
      <div className="flex flex-wrap items-center gap-2">
        <Pill tone="ok">一直有效</Pill>
        <span className="text-sm text-ink-2">管理员账号不受服务期限制。</span>
      </div>
    )
  }
  const left = daysLeft(me.service_until)
  if (me.service_active && left != null) {
    return (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Pill tone={left <= 7 ? 'warn' : 'ok'}>有效</Pill>
        <span className="text-[15px] text-ink tabular-nums">
          到 <strong className="font-semibold">{fmtDate(me.service_until)}</strong>，还剩 {left} 天
        </span>
      </div>
    )
  }
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
      <Pill tone="bad">已到期</Pill>
      <span className="text-[15px] text-ink-2 tabular-nums">
        {me.service_until ? `${fmtDate(me.service_until)} 到期，小鱼已经停下，数据都还在。` : '还没有开通服务。'}
      </span>
    </div>
  )
}

function RedeemForm({ me }: { me: User }) {
  const qc = useQueryClient()
  const loc = useLocation()
  const inputRef = useRef<HTMLInputElement>(null)
  const sectionRef = useRef<HTMLElement>(null)
  const [code, setCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const complete = /^FISH-[0-9A-Z]{4}-[0-9A-Z]{4}$/.test(code)

  useEffect(() => {
    if (loc.hash !== '#redeem') return
    const t = setTimeout(() => {
      sectionRef.current?.scrollIntoView({ block: 'center' })
      inputRef.current?.focus({ preventScroll: true })
    }, 60)
    return () => clearTimeout(t)
  }, [loc.hash, loc.key])

  async function submit(e: FormEvent) {
    e.preventDefault()
    if (!complete) {
      setError('邀请码是 FISH 加 8 位字母数字，比如 FISH-7K2M-Q9TD。')
      inputRef.current?.focus()
      return
    }
    setBusy(true)
    setError(null)
    try {
      const { user } = await api.redeem(code)
      qc.setQueryData(keys.me, user)
      qc.invalidateQueries({ queryKey: keys.gateway })
      qc.invalidateQueries({ queryKey: keys.llmKeys })
      setCode('')
      toast.success(`兑换成功，服务延长到 ${fmtDate(user.service_until)}`, {
        description: me.service_active ? undefined : '小鱼不会自动醒来，到我的小鱼里点启动就好。',
      })
    } catch (err) {
      if (err instanceof ApiError && inviteErrors[err.code]) setError(inviteErrors[err.code])
      else setError(err instanceof Error ? err.message : '没能兑换，稍后再试')
      inputRef.current?.focus()
    } finally {
      setBusy(false)
    }
  }

  return (
    <section ref={sectionRef} aria-labelledby="redeem-title" className="scroll-mt-24 rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <h2 id="redeem-title" className="flex items-center gap-2 font-display text-lg font-black text-ink">
        <Ticket className="size-5 text-pink-ink" aria-hidden />
        兑换邀请码
      </h2>
      <p className="mt-1 text-sm leading-relaxed text-ink-2">
        未到期时从到期日续期，已到期时从今天算起。
      </p>
      <form onSubmit={submit} noValidate className="mt-4">
        <Field label="邀请码" htmlFor="redeem" error={error}>
          <div className="flex gap-2">
            <Input
              ref={inputRef}
              id="redeem"
              value={code}
              onChange={(e) => {
                setCode(formatInvite(e.target.value))
                setError(null)
              }}
              placeholder="FISH-XXXX-XXXX"
              autoComplete="off"
              autoCapitalize="characters"
              spellCheck={false}
              maxLength={16}
              aria-invalid={!!error}
              className="min-w-0 flex-1 font-mono tracking-wider uppercase"
            />
            <Button type="submit" variant="primary" loading={busy} className="h-11">
              兑换
            </Button>
          </div>
        </Field>
      </form>
    </section>
  )
}

/** Expiry of a credit as a local date, with how many days are left when it is soon. */
function creditExpiry(iso: string | null): string {
  if (!iso) return '不过期'
  const ms = new Date(iso).getTime()
  const left = daysLeft(ms)
  return left != null && left <= 7 ? `${fmtDate(ms)} 过期，还剩 ${Math.max(left, 0)} 天` : `${fmtDate(ms)} 过期`
}

function EventCredit() {
  const qc = useQueryClient()
  const q = useGateway()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const g = q.data
  if (!g || (!g.account && g.pending.length === 0)) return null
  const b = g.balance
  const credits = b?.credits ?? []
  const k = g.key

  async function rotate() {
    setBusy(true)
    try {
      await api.rotateGateway()
      qc.invalidateQueries({ queryKey: keys.gateway })
      qc.invalidateQueries({ queryKey: keys.llmKeys })
      toast.success(k ? 'Key 已刷新，旧 Key 已失效' : '模型 Key 已重新加上')
      setOpen(false)
    } catch (err) {
      toast.error(err instanceof Error ? err.message : '没能刷新，稍后再试')
    } finally {
      setBusy(false)
    }
  }

  return (
    <section aria-labelledby="acct-event" className="rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <h2 id="acct-event" className="flex items-center gap-2 font-display text-lg font-black text-ink">
          <Coins className="size-5 text-pink-ink" aria-hidden />
          活动额度
        </h2>
        <p className={`font-display text-2xl font-black tabular-nums ${b?.disabled ? 'text-bad' : 'text-ink'}`}>
          {b ? `${b.available} 元` : q.isFetching ? <Skeleton className="h-8 w-24" /> : '查不到'}
        </p>
      </div>
      <p className="mt-1 text-sm leading-relaxed text-ink-2">活动送的模型额度，每笔到期自动作废，用的时候先扣最早过期的那笔。</p>
      {g.balance_error && <p className="mt-2 text-[13px] text-bad">{g.balance_error}</p>}
      {b?.disabled && <p className="mt-2 text-[13px] text-bad">这个 Key 已停用，请联系管理员。</p>}

      {(credits.length > 0 || g.pending.length > 0) && (
        <ul className="mt-3 divide-y divide-paper-3 border-y border-paper-3">
          {credits.map((c, i) => (
            <li key={`c${i}`} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-0.5 py-2.5">
              <span className="font-semibold text-ink tabular-nums">
                {c.available} 元
                <span className="ml-2 text-[13px] font-normal text-ink-3">共 {c.amount} 元</span>
              </span>
              <span className="text-[13px] text-ink-2 tabular-nums">{creditExpiry(c.expires_at)}</span>
            </li>
          ))}
          {g.pending.map((p, i) => (
            <li key={`p${i}`} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-0.5 py-2.5">
              <span className="font-semibold text-ink-2 tabular-nums">{p.amount} 元</span>
              <Pill tone="warn">发放中</Pill>
            </li>
          ))}
        </ul>
      )}
      {b && credits.length === 0 && g.pending.length === 0 && <p className="mt-3 text-[13px] text-ink-3">没有可用的额度了。</p>}

      {k ? (
        <p className="mt-3 flex flex-wrap gap-x-3 break-all text-sm text-ink-3">
          <span>{k.name}</span>
          <span>{k.model}</span>
          <code>{k.key_masked}</code>
        </p>
      ) : (
        g.account && <p className="mt-3 text-[13px] text-ink-3">模型 Key 已被删除，刷新后会重新加上。</p>
      )}
      <div className="mt-2 flex flex-wrap gap-2">
        <Button size="sm" variant="ghost" onClick={() => q.refetch()} loading={q.isFetching}>
          <RefreshCw aria-hidden />
          更新余额
        </Button>
        {g.account && (
          <Button size="sm" variant="ghost" onClick={() => setOpen(true)}>
            <KeyRound aria-hidden />
            刷新 Key
          </Button>
        )}
        <Link to="/app/keys" className="ml-auto self-center text-sm font-semibold text-sky">
          管理模型 Key
        </Link>
      </div>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title="刷新活动额度的 Key？"
        body="会换成一个新的 Key，旧 Key 立刻失效，余额不变。用它的小鱼会自动换上新 Key。"
        confirmLabel="刷新 Key"
        onConfirm={rotate}
        loading={busy}
        tone="primary"
      />
    </section>
  )
}

export default function AccountPage() {
  const { data: me } = useMe()
  const bots = useBots()
  const qc = useQueryClient()
  const navigate = useNavigate()

  if (!me) {
    return (
      <div className="grid gap-4">
        <Skeleton className="h-9 w-40" />
        <Skeleton className="h-40" />
      </div>
    )
  }

  async function logout() {
    try {
      await api.logout()
    } finally {
      qc.setQueryData(keys.me, null)
      qc.clear()
      navigate('/')
    }
  }

  const botCount = bots.data?.length
  const pluginCount = bots.data?.reduce((n, b) => n + b.plugins.length, 0)

  return (
    <>
      <PageHeader title="账号" />
      <div className="grid gap-6 lg:grid-cols-2 lg:items-start">
        <div className="grid gap-6">
          <section aria-labelledby="acct-profile" className="rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
            <h2 id="acct-profile" className="font-display text-lg font-black text-ink">
              基本信息
            </h2>
            <dl className="mt-4 grid grid-cols-[4.5em_minmax(0,1fr)] items-baseline gap-x-4 gap-y-3 text-[15px]">
              <dt className="text-ink-3">邮箱</dt>
              <dd className="break-all text-ink">{me.email}</dd>
              <dt className="text-ink-3">身份</dt>
              <dd className="text-ink">{me.role === 'admin' ? '管理员' : '普通用户'}</dd>
              <dt className="text-ink-3">注册时间</dt>
              <dd className="text-ink tabular-nums">{fmtDate(me.created_at)}</dd>
              <dt className="text-ink-3">服务</dt>
              <dd>
                <ServiceLine me={me} />
              </dd>
            </dl>
          </section>
          <RedeemForm me={me} />
        </div>

        <div className="grid gap-6">
          <section aria-labelledby="acct-quota" className="rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
            <div className="flex flex-wrap items-baseline justify-between gap-3">
              <h2 id="acct-quota" className="font-display text-lg font-black text-ink">
                额度
              </h2>
              <Link to="/app/usage" className="text-sm font-semibold text-sky">
                看用量明细
              </Link>
            </div>
            <dl className="mt-4 grid grid-cols-2 gap-x-4 gap-y-4">
              <Stat label="内存" value={fmtMb(me.quota.memory_mb)} />
              <Stat label="存储" value={fmtMb(me.quota.storage_mb)} />
              <Stat label="小鱼" value={me.quota.bots == null ? '不限' : `${me.quota.bots} 只`} sub={botCount != null ? `已托管 ${botCount} 只` : undefined} />
              <Stat label="插件" value={me.quota.plugins == null ? '不限' : `${me.quota.plugins} 个`} sub={pluginCount != null ? `已装 ${pluginCount} 个` : undefined} />
            </dl>
            <p className="mt-4 text-[13px] text-ink-3">调整额度请联系管理员。</p>
          </section>

          <EventCredit />

          <section aria-labelledby="acct-security" className="rounded-2xl border border-paper-3 bg-cream p-5 sm:p-6">
            <h2 id="acct-security" className="font-display text-lg font-black text-ink">
              登录与安全
            </h2>
            <div className="mt-4 flex flex-wrap gap-2">
              <Link
                to={`/reset?email=${encodeURIComponent(me.email)}`}
                className="inline-flex h-10 items-center gap-2 rounded-xl border border-paper-3 bg-cream px-4 text-sm font-semibold text-ink transition-colors duration-150 hover:border-ink-3/50 hover:bg-white"
              >
                <KeyRound className="size-4" aria-hidden />
                修改密码
              </Link>
              <Button variant="ghost" onClick={logout}>
                <LogOut aria-hidden />
                退出登录
              </Button>
            </div>
          </section>
        </div>
      </div>
    </>
  )
}
