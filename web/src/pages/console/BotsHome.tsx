import { useState, type FormEvent } from 'react'
import { Link, useLocation, useNavigate } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { ChevronRight, Plus, Puzzle } from 'lucide-react'
import { toast } from 'sonner'
import { api, ApiError } from '@/api/client'
import { keys, useBots, useLlmKeys, useMe, useUsage } from '@/api/hooks'
import { providerForEndpoint } from '@/api/modelProviders'
import type { Bot, CreateBotInput } from '@/api/types'
import { Button, ErrorNote, Field, Input, PageHeader, Skeleton } from '@/components/ui'
import { useFieldId } from '@/lib/fieldId'
import { Pupil } from '@/components/Pupil'
import { statusMood } from '@/components/mood'
import { FoodBar } from '@/components/FoodBar'
import { BotStatusPill, PlatformBadge } from './bot/status'
import { Select } from './bot/shared'
import ModelField from './ModelField'

function CreateBot({ reason, first }: { reason?: string; first?: boolean }) {
  const location = useLocation()
  const initial = (location.state as { newBot?: CreateBotInput } | null)?.newBot
  const [name, setName] = useState(initial?.name ?? '')
  const [keyId, setKeyId] = useState(initial?.key_id ?? '')
  const [model, setModel] = useState<string | null>(initial?.model || null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const qc = useQueryClient()
  const navigate = useNavigate()
  const keysQuery = useLlmKeys()
  const saved = keysQuery.data ?? []
  const selected = keyId ? saved.find((key) => key.id === keyId) : saved.length === 1 ? saved[0] : undefined
  const chosenModel = model ?? selected?.model ?? ''
  const nameTooLong = [...name.trim()].length > 32
  const modelTooLong = [...chosenModel.trim()].length > 100
  const disabled = !!reason || busy
  const canCreate = !disabled && !keysQuery.error && !!selected && !!chosenModel.trim() && !nameTooLong && !modelTooLong
  const nameId = useFieldId('new-fish-name')
  const keyFieldId = useFieldId('new-fish-key')
  const modelId = useFieldId('new-fish-model')
  const draft = { name, key_id: selected?.id ?? '', model: chosenModel }
  const provider = selected && providerForEndpoint(selected.base_url)
  const addKey = (
    <Link
      to="/app/keys"
      state={{ newBot: draft }}
      className="inline-flex min-h-10 items-center gap-1 rounded-lg px-2 text-sm font-semibold text-sky underline-offset-2 hover:underline"
    >
      <Plus className="size-4" aria-hidden /> 添加 Key
    </Link>
  )

  async function submit(e: FormEvent) {
    e.preventDefault()
    if (!canCreate || !selected) return
    setBusy(true)
    setError('')
    try {
      const bot = await api.createBot({ name: name.trim() || '大肥鱼', key_id: selected.id, model: chosenModel.trim() })
      qc.invalidateQueries({ queryKey: keys.bots })
      toast.success(`${bot.name} 已创建，接下来可以绑定聊天平台`)
      navigate(`/app/bots/${bot.id}/connect`, { replace: !!initial })
    } catch (err) {
      setError(err instanceof ApiError ? err.message : '没能创建，稍后再试')
      if (err instanceof ApiError && err.status === 404) void qc.invalidateQueries({ queryKey: keys.llmKeys })
    } finally {
      setBusy(false)
    }
  }

  return (
    <form onSubmit={submit} className="grid min-w-0 gap-4">
      <Field
        label={first ? '给第一条小鱼起个名字' : '新小鱼的名字'}
        htmlFor={nameId}
        error={nameTooLong ? '名字最多 32 个字符' : undefined}
      >
        <Input
          id={nameId}
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="大肥鱼"
          aria-invalid={nameTooLong || undefined}
          disabled={disabled}
        />
      </Field>
      {keysQuery.isPending ? (
        <Skeleton className="h-16" />
      ) : keysQuery.error ? (
        <ErrorNote error={keysQuery.error} onRetry={() => keysQuery.refetch()} />
      ) : saved.length === 0 ? (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-paper-3 px-3.5 py-1.5">
          <p className="text-sm text-ink-2">先添加一个模型 Key。</p>
          {!disabled && addKey}
        </div>
      ) : (
        <div className="grid min-w-0 items-end gap-4 md:grid-cols-2">
          <Field label="模型 Key" htmlFor={keyFieldId} aside={!disabled && addKey}>
            <Select
              id={keyFieldId}
              value={selected?.id ?? ''}
              disabled={disabled}
              onChange={(event) => {
                setKeyId(event.target.value)
                setModel(null)
                setError('')
              }}
              required
            >
              <option value="" disabled>
                选择已保存的 Key
              </option>
              {saved.map((key) => (
                <option key={key.id} value={key.id}>
                  {key.name}
                </option>
              ))}
            </Select>
          </Field>
          {selected ? (
            <ModelField
              key={selected.id}
              id={modelId}
              value={chosenModel}
              onChange={setModel}
              models={[selected.model, ...(provider?.models ?? [])]}
              disabled={disabled}
              error={modelTooLong ? '模型名最多 100 个字符' : undefined}
            />
          ) : (
            <Field label="模型名" htmlFor={modelId}>
              <Select id={modelId} disabled>
                <option>先选择模型 Key</option>
              </Select>
            </Field>
          )}
        </div>
      )}
      {error && (
        <p role="alert" className="text-sm text-bad">
          {error}
        </p>
      )}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {reason && <p className="mr-auto text-[13px] text-ink-3">{reason}</p>}
        <Button type="submit" variant="primary" loading={busy} disabled={!canCreate}>
          <Plus aria-hidden />
          {first ? '开始托管' : '添加小鱼'}
        </Button>
      </div>
    </form>
  )
}

function BotRow({ bot }: { bot: Bot }) {
  const running = bot.plugins.filter((p) => p.status === 'running' || p.status === 'sleeping').length
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
          {bot.status_detail && bot.status !== 'running' && <p className="mt-1 line-clamp-1 text-[13px] text-ink-2">{bot.status_detail}</p>}
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
  const reason = expired ? '服务已到期，兑换邀请码后才能收新的小鱼。' : full ? `你的名额是 ${quota} 条小鱼，已经满了。` : undefined

  return (
    <>
      <PageHeader title="我的小鱼" />

      {bots.isLoading ? (
        <div className="grid gap-3">
          <Skeleton className="h-28" />
          <Skeleton className="h-28" />
        </div>
      ) : bots.error ? (
        <ErrorNote error={bots.error} onRetry={() => bots.refetch()} />
      ) : list.length === 0 ? (
        <section className="grid min-w-0 items-start gap-6 rounded-2xl border border-paper-3 bg-cream p-5 sm:grid-cols-[auto_minmax(0,1fr)] sm:p-7">
          <Pupil mood="happy" size={112} className="mx-auto" />
          <div>
            <h2 className="font-display text-xl font-black text-ink">创建第一条小鱼</h2>
            <p className="mt-1 text-sm text-ink-2">创建后就可以接入 QQ 或微信。</p>
            <div className="mt-5">
              <CreateBot reason={reason} first />
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
            <CreateBot reason={reason} />
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
              servings={usage.data.sources.map((s) => ({
                key: `${s.bot_id}-${s.kind}-${s.plugin_id}`,
                label: s.label,
                bytes: s.memory_bytes,
              }))}
              compact
            />
          </div>
        </section>
      )}
    </>
  )
}
