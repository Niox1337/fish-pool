import { useState, type FormEvent } from 'react'
import { Link, useNavigate } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronRight, Plus, Puzzle } from 'lucide-react'
import { toast } from 'sonner'
import { api, ApiError } from '@/api/client'
import { keys, useBots, useMe, useUsage } from '@/api/hooks'
import type { Bot } from '@/api/types'
import { Button, ErrorNote, Input, PageHeader, Skeleton } from '@/components/ui'
import { Pupil, statusMood } from '@/components/Pupil'
import { FoodBar } from '@/components/FoodBar'
import { BotStatusPill, PlatformBadge } from './bot/status'

function CreateBot({ disabled, reason, first }: { disabled: boolean; reason?: string; first?: boolean }) {
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const qc = useQueryClient()
  const navigate = useNavigate()

  async function submit(e: FormEvent) {
    e.preventDefault()
    setBusy(true)
    try {
      const bot = await api.createBot(name.trim() || '大肥鱼')
      qc.invalidateQueries({ queryKey: keys.bots })
      toast.success(`${bot.name} 已创建，接下来可以绑定聊天平台`)
      navigate(`/app/bots/${bot.id}/connect`)
    } catch (err) {
      toast.error(err instanceof ApiError ? err.message : '没能创建，稍后再试')
    } finally {
      setBusy(false)
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-wrap items-end gap-2">
      <label className="grid min-w-[200px] flex-1 gap-1.5">
        <span className="text-sm font-semibold text-ink">{first ? '给第一条小鱼起个名字' : '新小鱼的名字'}</span>
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="大肥鱼" maxLength={24} disabled={disabled} />
      </label>
      <Button type="submit" variant="primary" loading={busy} disabled={disabled} title={reason}>
        <Plus aria-hidden />
        {first ? '开始托管' : '添加小鱼'}
      </Button>
      {reason && <p className="basis-full text-[13px] text-ink-3">{reason}</p>}
    </form>
  )
}

function BotRow({ bot }: { bot: Bot }) {
  const running = bot.plugins.filter((p) => p.status === 'running').length
  const troubled = bot.plugins.filter((p) => p.status === 'error' || p.status === 'over_quota').length
  return (
    <li>
      <Link
        to={`/app/bots/${bot.id}`}
        className="group flex items-center gap-4 rounded-2xl border border-paper-3 bg-cream p-4 transition-[border-color,box-shadow] duration-150 hover:border-ink-3/40 hover:shadow-[var(--shadow-soft)] sm:gap-5 sm:p-5"
      >
        <Pupil mood={statusMood[bot.status]} size={72} className="rounded-2xl bg-paper-2" />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="truncate font-display text-lg font-black text-ink">{bot.name}</h2>
            <BotStatusPill status={bot.status} />
          </div>
          {bot.status_detail && bot.status !== 'running' && (
            <p className="mt-1 line-clamp-1 text-[13px] text-ink-2">{bot.status_detail}</p>
          )}
          <div className="mt-2 flex flex-wrap items-center gap-2 text-[13px] text-ink-2">
            {bot.platforms.length === 0 ? (
              <span className="text-warn">还没接入 QQ 或微信</span>
            ) : (
              bot.platforms.map((p) => <PlatformBadge key={p.id} platform={p} />)
            )}
            <span className="inline-flex items-center gap-1 tabular-nums">
              <Puzzle className="size-3.5" aria-hidden />
              {bot.plugins.length} 个插件{running < bot.plugins.length && bot.plugins.length > 0 ? `，${running} 个在运行` : ''}
            </span>
            {troubled > 0 && <span className="font-semibold text-bad tabular-nums">{troubled} 个插件需要看看</span>}
          </div>
        </div>
        <ChevronRight className="size-5 shrink-0 text-ink-3 transition-transform duration-150 group-hover:translate-x-0.5" aria-hidden />
      </Link>
    </li>
  )
}

export default function BotsHome() {
  const { data: me } = useMe()
  const bots = useBots()
  const usage = useUsage()

  const list = bots.data ?? []
  const quota = me?.quota.bots ?? null
  const full = quota != null && list.length >= quota
  const expired = me ? !me.service_active : false
  const reason = expired
    ? '服务已到期，兑换邀请码后才能收新的小鱼。'
    : full
      ? `你的名额是 ${quota} 条小鱼，已经满了。`
      : undefined

  return (
    <>
      <PageHeader title="我的小鱼" description="每条小鱼都是一个独立运行的机器人。点进去可以接入 QQ 或微信、换人设、装插件、看日志。" />

      {bots.isLoading ? (
        <div className="grid gap-3">
          <Skeleton className="h-28" />
          <Skeleton className="h-28" />
        </div>
      ) : bots.error ? (
        <ErrorNote error={bots.error} onRetry={() => bots.refetch()} />
      ) : list.length === 0 ? (
        <section className="grid items-center gap-6 rounded-3xl border border-paper-3 bg-cream p-6 sm:grid-cols-[auto_1fr] sm:p-8">
          <Pupil mood="happy" size={144} className="mx-auto" />
          <div>
            <h2 className="font-display text-xl font-black text-ink">还没有托管的小鱼</h2>
            <ol className="mt-3 grid gap-1.5 text-[15px] leading-relaxed text-ink-2">
              <li>1. 起个名字，创建第一只小鱼。</li>
              <li>2. 在接入页用 QQ 或微信扫码，凭证自动填好。</li>
              <li>3. 在模型 Key 页填上你自己的 Key，小鱼就能开口说话。</li>
            </ol>
            <div className="mt-5">
              <CreateBot disabled={!!reason} reason={reason} first />
            </div>
          </div>
        </section>
      ) : (
        <>
          <ul className="grid gap-3">
            {list.map((b) => (
              <BotRow key={b.id} bot={b} />
            ))}
          </ul>
          <div className="mt-6 rounded-2xl border border-dashed border-paper-3 p-5">
            <CreateBot disabled={!!reason} reason={reason} />
          </div>
        </>
      )}

      {usage.data && list.length > 0 && (
        <section className="mt-10" aria-labelledby="home-usage">
          <div className="mb-3 flex items-baseline justify-between gap-3">
            <h2 id="home-usage" className="font-display text-lg font-black text-ink">
              饭量
            </h2>
            <Link to="/app/usage" className="text-sm font-semibold text-sky">
              看明细
            </Link>
          </div>
          <div className="rounded-2xl border border-paper-3 bg-cream p-5">
            <FoodBar
              label="内存"
              capacity={usage.data.quota.memory_mb * 1024 * 1024}
              servings={usage.data.sources.map((s) => ({ key: `${s.bot_id}-${s.kind}-${s.plugin_id}`, label: s.label, bytes: s.memory_bytes }))}
              compact
            />
          </div>
        </section>
      )}
    </>
  )
}
