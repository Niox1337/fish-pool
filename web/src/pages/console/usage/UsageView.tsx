import { Fragment, useMemo } from 'react'
import { Area, AreaChart, CartesianGrid, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import type { Bot, Quota, Usage, UsageSource } from '@/api/types'
import { FoodBar, type Serving } from '@/components/FoodBar'
import { Skeleton, Stat } from '@/components/ui'
import { PluginStatusPill } from '@/pages/console/bot/status'
import { cn } from '@/lib/cn'
import { fmtBytes, fmtDateTime, fmtTime } from '@/lib/format'

const MB = 1024 * 1024

const kindLabel = { core: '小鱼本体', conversations: '聊天记录' } as const

function sourceLabel(s: UsageSource) {
  return s.kind === 'plugin' ? s.label : kindLabel[s.kind]
}

function sourceKey(s: UsageSource) {
  return `${s.bot_id}-${s.kind}-${s.plugin_id ?? ''}`
}

type Group = { botId: string; name: string; bot?: Bot; rows: UsageSource[]; memory: number; storage: number; cpu: number }

function groupSources(sources: UsageSource[], bots?: Bot[]): Group[] {
  const map = new Map<string, Group>()
  for (const s of sources) {
    let g = map.get(s.bot_id)
    if (!g) {
      const bot = bots?.find((b) => b.id === s.bot_id)
      g = { botId: s.bot_id, name: bot?.name ?? `小鱼 ${s.bot_id.slice(0, 6)}`, bot, rows: [], memory: 0, storage: 0, cpu: 0 }
      map.set(s.bot_id, g)
    }
    g.rows.push(s)
    g.memory += s.memory_bytes
    g.storage += s.storage_bytes
    g.cpu += s.cpu_percent
  }
  const groups = [...map.values()]
  for (const g of groups) g.rows.sort((a, b) => b.memory_bytes - a.memory_bytes || b.storage_bytes - a.storage_bytes)
  return groups.sort((a, b) => b.memory - a.memory)
}

const fmtCpu = (n: number) => `${n.toFixed(1)}%`

function Breakdown({ groups }: { groups: Group[] }) {
  const multi = groups.length > 1
  return (
    <div className="overflow-hidden rounded-2xl border border-paper-3 bg-cream">
      <table className="w-full table-fixed text-[13px] sm:text-sm">
        <caption className="sr-only">按小鱼和来源分组的资源用量，按内存从多到少排列</caption>
        <colgroup>
          <col />
          <col className="w-[22%] sm:w-32" />
          <col className="w-[22%] sm:w-32" />
          <col className="w-[16%] sm:w-24" />
        </colgroup>
        <thead>
          <tr className="border-b border-paper-3 text-left text-[12px] text-ink-3 sm:text-[13px]">
            <th scope="col" className="px-3 py-2.5 font-semibold sm:px-4">
              来源
            </th>
            <th scope="col" className="px-2 py-2.5 text-right font-semibold">
              内存
            </th>
            <th scope="col" className="px-2 py-2.5 text-right font-semibold">
              存储
            </th>
            <th scope="col" className="py-2.5 pr-3 pl-2 text-right font-semibold sm:pr-4">
              CPU
            </th>
          </tr>
        </thead>
        <tbody>
          {groups.map((g) => (
            <Fragment key={g.botId}>
              {(multi || g.bot) && (
                <tr className="border-b border-paper-3 bg-paper-2/60">
                  <th scope="rowgroup" className="truncate px-3 py-2 text-left font-semibold text-ink sm:px-4">
                    {g.name}
                  </th>
                  <td className="px-2 py-2 text-right font-semibold tabular-nums text-ink">{fmtBytes(g.memory)}</td>
                  <td className="px-2 py-2 text-right font-semibold tabular-nums text-ink">{fmtBytes(g.storage)}</td>
                  <td className="py-2 pr-3 pl-2 text-right font-semibold tabular-nums text-ink sm:pr-4">{fmtCpu(g.cpu)}</td>
                </tr>
              )}
              {g.rows.map((s) => {
                const plugin = s.plugin_id ? g.bot?.plugins.find((p) => p.id === s.plugin_id) : undefined
                const flagged = plugin && plugin.status !== 'running'
                return (
                  <tr key={sourceKey(s)} className="border-b border-paper-3/70 last:border-b-0">
                    <th scope="row" className="px-3 py-2 text-left font-normal text-ink-2 sm:px-4">
                      <span className={cn('flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5', (multi || g.bot) && 'pl-3')}>
                        <span className="min-w-0 truncate">{sourceLabel(s)}</span>
                        {flagged && <PluginStatusPill status={plugin.status} />}
                      </span>
                    </th>
                    <td className="px-2 py-2 text-right tabular-nums text-ink">{fmtBytes(s.memory_bytes)}</td>
                    <td className="px-2 py-2 text-right tabular-nums text-ink">{fmtBytes(s.storage_bytes)}</td>
                    <td className="py-2 pr-3 pl-2 text-right tabular-nums text-ink-2 sm:pr-4">{fmtCpu(s.cpu_percent)}</td>
                  </tr>
                )
              })}
            </Fragment>
          ))}
        </tbody>
      </table>
    </div>
  )
}

type Point = Usage['history'][number]

function ChartTip({ active, payload, field, label }: { active?: boolean; payload?: { payload: Point }[]; field: 'memory_bytes' | 'storage_bytes'; label: string }) {
  const p = payload?.[0]?.payload
  if (!active || !p) return null
  return (
    <div className="rounded-xl border border-paper-3 bg-cream px-3 py-2 text-[13px] shadow-[var(--shadow-soft)]">
      <p className="text-ink-3 tabular-nums">{fmtDateTime(p.ts)}</p>
      <p className="mt-0.5 font-semibold text-ink tabular-nums">
        {label} {fmtBytes(p[field])}
      </p>
    </div>
  )
}

function HistoryChart({
  data,
  field,
  label,
  quotaBytes,
  color,
}: {
  data: Point[]
  field: 'memory_bytes' | 'storage_bytes'
  label: string
  quotaBytes: number
  color: string
}) {
  const peak = data.reduce((m, p) => Math.max(m, p[field]), 0)
  const latest = data.at(-1)?.[field] ?? 0
  const showQuota = quotaBytes <= peak * 1.6
  return (
    <figure className="min-w-0 rounded-2xl border border-paper-3 bg-cream p-4">
      <figcaption className="mb-3 flex flex-wrap items-baseline justify-between gap-x-3 text-sm">
        <span className="font-semibold text-ink">{label}</span>
        <span className="text-[13px] text-ink-3 tabular-nums">
          现在 {fmtBytes(latest)}，峰值 {fmtBytes(peak)}，额度 {fmtBytes(quotaBytes)}
        </span>
      </figcaption>
      <div className="h-44 sm:h-52" role="img" aria-label={`过去 24 小时${label}用量，峰值 ${fmtBytes(peak)}`}>
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={data} margin={{ top: 6, right: 6, bottom: 0, left: 0 }}>
            <CartesianGrid vertical={false} stroke="var(--color-paper-3)" strokeDasharray="3 4" />
            <XAxis
              dataKey="ts"
              type="number"
              domain={['dataMin', 'dataMax']}
              scale="time"
              tickFormatter={(t: number) => fmtTime(t).slice(0, 5)}
              tick={{ fill: 'var(--color-ink-3)', fontSize: 12 }}
              tickLine={false}
              axisLine={{ stroke: 'var(--color-paper-3)' }}
              minTickGap={36}
            />
            <YAxis
              width={64}
              tickFormatter={(v: number) => fmtBytes(v)}
              tick={{ fill: 'var(--color-ink-3)', fontSize: 12 }}
              tickLine={false}
              axisLine={false}
              domain={[0, (max: number) => Math.max(max * 1.15, showQuota ? quotaBytes * 1.05 : 0, MB)]}
              tickCount={4}
            />
            <Tooltip
              cursor={{ stroke: 'var(--color-ink-3)', strokeDasharray: '3 3' }}
              content={(props) => <ChartTip active={props.active} payload={props.payload as unknown as { payload: Point }[]} field={field} label={label} />}
            />
            {showQuota && (
              <ReferenceLine
                y={quotaBytes}
                stroke="var(--color-bad)"
                strokeDasharray="5 4"
                label={{ value: '额度', position: 'insideTopRight', fill: 'var(--color-bad)', fontSize: 12 }}
              />
            )}
            <Area
              type="monotone"
              dataKey={field}
              stroke={color}
              strokeWidth={2}
              fill={color}
              fillOpacity={0.12}
              isAnimationActive={false}
              activeDot={{ r: 4, strokeWidth: 2, stroke: 'var(--color-cream)' }}
            />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </figure>
  )
}

export function UsageView({ usage, bots, quota }: { usage: Usage; bots?: Bot[]; quota?: Quota }) {
  const groups = useMemo(() => groupSources(usage.sources, bots), [usage.sources, bots])
  const multi = groups.length > 1
  const servings = (field: 'memory_bytes' | 'storage_bytes'): Serving[] =>
    usage.sources.map((s) => {
      const g = groups.find((x) => x.botId === s.bot_id)
      return { key: sourceKey(s), label: multi && g ? `${g.name}的${sourceLabel(s)}` : sourceLabel(s), bytes: s[field] }
    })
  const pluginCount = bots?.reduce((n, b) => n + b.plugins.length, 0)
  const memQuota = usage.quota.memory_mb * MB
  const storeQuota = usage.quota.storage_mb * MB

  return (
    <div className="grid gap-8">
      <section aria-labelledby="usage-now" className="grid gap-4">
        <h2 id="usage-now" className="font-display text-lg font-black text-ink">
          现在的饭量
        </h2>
        <div className="grid gap-5 rounded-2xl border border-paper-3 bg-cream p-5">
          <FoodBar label="内存" capacity={memQuota} servings={servings('memory_bytes')} />
          <FoodBar label="存储" capacity={storeQuota} servings={servings('storage_bytes')} />
          {bots && quota && (
            <dl className="grid grid-cols-2 gap-4 border-t border-paper-3 pt-4 sm:max-w-md">
              <Stat label="小鱼" value={`${bots.length} / ${quota.bots ?? '不限'}`} tone={quota.bots != null && bots.length > quota.bots ? 'bad' : undefined} />
              <Stat
                label="插件（所有小鱼合计）"
                value={`${pluginCount} / ${quota.plugins ?? '不限'}`}
                tone={quota.plugins != null && (pluginCount ?? 0) > quota.plugins ? 'bad' : undefined}
              />
            </dl>
          )}
        </div>
        <p className="text-[13px] leading-relaxed text-ink-2">
          内存持续超额会停用占用最大的插件。存储超额时，请先清理插件或聊天记录，再安装新插件。
        </p>
      </section>

      <section aria-labelledby="usage-breakdown" className="grid gap-4">
        <h2 id="usage-breakdown" className="font-display text-lg font-black text-ink">
          谁吃了多少
        </h2>
        {groups.length === 0 ? (
          <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-[15px] text-ink-2">
            暂无用量。启动小鱼后查看明细。
          </p>
        ) : (
          <Breakdown groups={groups} />
        )}
      </section>

      <section aria-labelledby="usage-history" className="grid gap-4">
        <h2 id="usage-history" className="font-display text-lg font-black text-ink">
          过去 24 小时
        </h2>
        {usage.history.length === 0 || (usage.sources.length === 0 && usage.history.every((h) => h.memory_bytes === 0 && h.storage_bytes === 0)) ? (
          <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-[15px] text-ink-2">
            暂无记录。小鱼运行后每 5 分钟采样。
          </p>
        ) : (
          <div className="grid gap-4 lg:grid-cols-2">
            <HistoryChart data={usage.history} field="memory_bytes" label="内存" quotaBytes={memQuota} color="var(--color-cobalt)" />
            <HistoryChart data={usage.history} field="storage_bytes" label="存储" quotaBytes={storeQuota} color="var(--color-pink-ink)" />
          </div>
        )}
      </section>
    </div>
  )
}

export function UsageSkeleton() {
  return (
    <div className="grid gap-6" aria-busy>
      <Skeleton className="h-6 w-32" />
      <Skeleton className="h-44" />
      <Skeleton className="h-56" />
    </div>
  )
}
