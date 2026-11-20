import { useState, type ReactNode } from 'react'
import { Link } from 'react-router'
import { RefreshCw } from 'lucide-react'
import { CartesianGrid, Line, LineChart, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import { useAdminSystem, SYSTEM_REFRESH_MS } from '@/api/hooks'
import type { AdminSystem as System, AdminSystemNode, NodeSystem, SysCounts, SysPoint, SysProcess } from '@/api/types'
import { Button, ErrorNote, PageHeader, Pill, Skeleton } from '@/components/ui'
import { cn } from '@/lib/cn'
import { fmtAgo, fmtBytes, fmtTime, pct } from '@/lib/format'

const SERIES = { container: '#3d55d6', host: '#c46a12' }

const n = (v: number) => v.toLocaleString('zh-CN')
const cores = (percent: number | null | undefined) => (percent == null ? '无数据' : `${(percent / 100).toFixed(2)} 核`)

function fmtDuration(seconds: number | null | undefined): string {
  if (seconds == null || seconds < 0) return '无数据'
  const d = Math.floor(seconds / 86400)
  const h = Math.floor((seconds % 86400) / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  if (d > 0) return `${d} 天 ${h} 小时`
  if (h > 0) return `${h} 小时 ${m} 分`
  return m > 0 ? `${m} 分钟` : '不到 1 分钟'
}

const since = (ms: number | null | undefined) => (ms ? fmtDuration((Date.now() - ms) / 1000) : '无数据')

function Section({ id, title, aside, children, className }: { id: string; title: string; aside?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section aria-labelledby={id} className={className}>
      <div className="mb-3 flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <h2 id={id} className="font-display text-lg font-black text-ink">{title}</h2>
        {aside && <div className="text-[13px] text-ink-3">{aside}</div>}
      </div>
      {children}
    </section>
  )
}

const panel = 'rounded-2xl border border-paper-3 bg-cream'

function Meter({ value, label }: { value: number; label: string }) {
  const tone = value > 85 ? 'bg-bad' : value > 70 ? 'bg-star-2' : 'bg-cobalt'
  return (
    <span role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(value)}
      className="block h-1.5 w-full overflow-hidden rounded-full bg-paper-2">
      <span className={cn('block h-full rounded-full transition-[width] duration-300 ease-[var(--ease-out-expo)]', tone)} style={{ width: `${value}%` }} />
    </span>
  )
}

function Readout({ label, value, of, meter, sub, className }: {
  label: string
  value: ReactNode
  of?: ReactNode
  meter?: number | null
  sub?: ReactNode
  className?: string
}) {
  return (
    <div className={cn('grid content-start gap-1.5 bg-cream p-4', className)}>
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-[13px] text-ink-3">{label}</span>
        {meter != null && <span className="text-[13px] font-semibold tabular-nums text-ink-2">{meter.toFixed(0)}%</span>}
      </div>
      <p className="text-lg font-bold tabular-nums text-ink">
        {value}
        {of && <span className="text-sm font-semibold text-ink-3"> / {of}</span>}
      </p>
      {meter != null && <Meter value={meter} label={label} />}
      {sub && <p className="text-[13px] leading-relaxed text-ink-3 tabular-nums">{sub}</p>}
    </div>
  )
}

function Resources({ s }: { s: NodeSystem }) {
  const h = s.host
  const c = s.container
  const hostUsed = h?.memory_available_bytes != null ? h.memory_total_bytes - h.memory_available_bytes : null
  const diskUsed = s.disk ? s.disk.total_bytes - s.disk.free_bytes : null
  return (
    <div className={cn(panel, 'grid gap-px overflow-hidden bg-paper-3 sm:grid-cols-2 lg:grid-cols-5')}>
      <Readout
        label="宿主机 CPU"
        value={cores(h?.cpu_percent)}
        of={h ? `${h.cpu_count} 核` : undefined}
        meter={h?.cpu_percent != null ? pct(h.cpu_percent, h.cpu_count * 100) : null}
        sub={h?.load ? `负载 ${h.load.map((v) => v.toFixed(2)).join(' · ')}` : undefined}
      />
      <Readout
        label="宿主机内存"
        value={fmtBytes(hostUsed)}
        of={h ? fmtBytes(h.memory_total_bytes) : undefined}
        meter={hostUsed != null && h ? pct(hostUsed, h.memory_total_bytes) : null}
        sub={h ? `可用 ${fmtBytes(h.memory_available_bytes)} · 交换 ${h.swap_total_bytes ? fmtBytes((h.swap_total_bytes ?? 0) - (h.swap_free_bytes ?? 0)) : '未开启'}` : undefined}
      />
      <Readout
        label="容器内存"
        value={fmtBytes(c?.memory_current_bytes)}
        of={c?.memory_max_bytes ? fmtBytes(c.memory_max_bytes) : c ? '不限' : undefined}
        meter={c?.memory_max_bytes ? pct(c.memory_current_bytes, c.memory_max_bytes) : null}
        sub={c ? (
          <>
            峰值 {fmtBytes(c.memory_peak_bytes)} · 匿名 {fmtBytes(c.memory_anon_bytes)} · 文件缓存 {fmtBytes(c.memory_file_bytes)}
            {c.oom_kills > 0 && <span className="font-semibold text-bad"> · OOM {c.oom_kills} 次</span>}
          </>
        ) : '节点不在 cgroup 容器中'}
      />
      <Readout
        label="容器 CPU"
        value={cores(c?.cpu_percent)}
        of={c ? (c.cpu_limit_cores ? `${c.cpu_limit_cores.toFixed(2)} 核` : '不限') : undefined}
        meter={c?.cpu_percent != null && c.cpu_limit_cores ? pct(c.cpu_percent, c.cpu_limit_cores * 100) : null}
        sub={c?.pids_current != null ? `进程与线程 ${n(c.pids_current)}${c.pids_max ? ` / ${n(c.pids_max)}` : ''}` : undefined}
      />
      <Readout
        label={s.disk ? `磁盘 ${s.disk.path}` : '磁盘'}
        className="sm:col-span-2 lg:col-span-1"
        value={fmtBytes(diskUsed)}
        of={s.disk ? fmtBytes(s.disk.total_bytes) : undefined}
        meter={s.disk && diskUsed != null ? pct(diskUsed, s.disk.total_bytes) : null}
        sub={s.disk ? `可写 ${fmtBytes(s.disk.avail_bytes)} · inode ${pct(s.disk.inodes_total - s.disk.inodes_free, s.disk.inodes_total).toFixed(0)}%` : undefined}
      />
    </div>
  )
}

type SeriesDef = { key: keyof SysPoint; label: string; color: string }

function TrendTip({ active, payload, series, format }: { active?: boolean; payload?: { payload: SysPoint }[]; series: SeriesDef[]; format: (v: number) => string }) {
  if (!active || !payload?.length) return null
  const p = payload[0].payload
  return (
    <div className="rounded-xl border border-paper-3 bg-white px-3 py-2 text-[13px] shadow-[var(--shadow-soft)]">
      <p className="tabular-nums text-ink-3">{fmtTime(p.ts)}</p>
      {series.map((s) => (
        <p key={s.key} className="flex items-center gap-2 tabular-nums text-ink">
          <span className="size-2 rounded-full" style={{ background: s.color }} aria-hidden />
          {s.label} <strong className="ml-auto pl-3 font-semibold">{p[s.key] == null ? '无数据' : format(p[s.key] as number)}</strong>
        </p>
      ))}
    </div>
  )
}

function Trend({ id, title, data, series, format, tick, floor, reference }: {
  id: string
  title: string
  data: SysPoint[]
  series: SeriesDef[]
  format: (v: number) => string
  tick: (v: number) => string
  floor: number
  reference?: { y: number; label: string }
}) {
  const enough = data.filter((d) => series.some((s) => d[s.key] != null)).length >= 2
  return (
    <figure aria-labelledby={id} className={cn(panel, 'p-4')}>
      <figcaption className="mb-2 flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
        <span id={id} className="text-sm font-semibold text-ink">{title}</span>
        <span className="flex gap-3 text-[13px] text-ink-2">
          {series.map((s) => (
            <span key={s.key} className="inline-flex items-center gap-1.5">
              <span className="h-0.5 w-3.5 rounded-full" style={{ background: s.color }} aria-hidden />
              {s.label}
            </span>
          ))}
        </span>
      </figcaption>
      {enough ? (
        <div className="h-44">
          <ResponsiveContainer width="100%" height="100%">
            <LineChart data={data} margin={{ top: 6, right: 8, bottom: 0, left: 0 }}>
              <CartesianGrid vertical={false} stroke="var(--color-paper-3)" strokeDasharray="3 4" />
              <XAxis dataKey="ts" type="number" scale="time" domain={['dataMin', 'dataMax']}
                tickFormatter={(t: number) => fmtTime(t).slice(0, 5)} tick={{ fill: 'var(--color-ink-3)', fontSize: 12 }}
                tickLine={false} axisLine={{ stroke: 'var(--color-paper-3)' }} minTickGap={40} />
              <YAxis width={58} tickFormatter={tick} tick={{ fill: 'var(--color-ink-3)', fontSize: 12 }} tickLine={false} axisLine={false}
                tickCount={4} domain={[0, (max: number) => Math.max(max * 1.15, floor, reference ? reference.y * 1.08 : 0)]} />
              <Tooltip cursor={{ stroke: 'var(--color-ink-3)', strokeDasharray: '3 3' }}
                content={(p) => <TrendTip active={p.active} payload={p.payload as unknown as { payload: SysPoint }[]} series={series} format={format} />} />
              {reference && (
                <ReferenceLine y={reference.y} stroke="var(--color-bad)" strokeDasharray="5 4"
                  label={{ value: reference.label, position: 'insideTopRight', fill: 'var(--color-bad)', fontSize: 12 }} />
              )}
              {series.map((s) => (
                <Line key={s.key} type="linear" dataKey={s.key} stroke={s.color} strokeWidth={2} dot={false}
                  activeDot={{ r: 4, stroke: 'var(--color-cream)', strokeWidth: 2 }} isAnimationActive={false} connectNulls={false} />
              ))}
            </LineChart>
          </ResponsiveContainer>
        </div>
      ) : (
        <p className="grid h-44 place-items-center rounded-xl border border-dashed border-paper-3 text-sm text-ink-3">
          正在采样，两次采样后出现曲线
        </p>
      )}
    </figure>
  )
}

function Trends({ s }: { s: NodeSystem }) {
  const limit = s.container?.memory_max_bytes
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Trend id="sys-trend-cpu" title="CPU（核）" data={s.history} floor={20}
        series={[{ key: 'container_cpu', label: '容器', color: SERIES.container }, { key: 'host_cpu', label: '宿主机', color: SERIES.host }]}
        format={(v) => cores(v)} tick={(v) => (v / 100).toFixed(2)} />
      <Trend id="sys-trend-mem" title="内存" data={s.history} floor={64 * 1024 * 1024}
        series={[{ key: 'container_memory_bytes', label: '容器', color: SERIES.container }, { key: 'host_memory_used_bytes', label: '宿主机已用', color: SERIES.host }]}
        format={fmtBytes} tick={(v) => fmtBytes(v)} reference={limit ? { y: limit, label: '容器上限' } : undefined} />
    </div>
  )
}

const roleLabel: Record<SysProcess['role'], string> = { node: '节点程序', sidecar: '插件宿主', plugin: '插件', task: '节点子任务', other: '其他' }
const roleTone: Record<SysProcess['role'], string> = {
  node: 'bg-sky-soft text-sky',
  sidecar: 'bg-pink-soft text-pink-ink',
  plugin: 'bg-ok-soft text-ok',
  task: 'bg-warn-soft text-warn',
  other: 'bg-paper-2 text-ink-2',
}

function RoleTag({ role }: { role: SysProcess['role'] }) {
  return <span className={cn('inline-block rounded-md px-1.5 py-px text-[12px] font-semibold whitespace-nowrap', roleTone[role])}>{roleLabel[role]}</span>
}

function Owner({ p, users }: { p: SysProcess; users: System['users'] }) {
  if (!p.plugin) return <span className="text-ink-3">平台</span>
  return (
    <span className="grid min-w-0">
      <span className="truncate text-ink">{p.plugin.bot_name}</span>
      <Link to={`/app/admin/users/${p.plugin.user_id}`} className="truncate text-[12px] text-sky underline-offset-2 hover:underline">
        {users[p.plugin.user_id] ?? p.plugin.user_id}
      </Link>
    </span>
  )
}

type SortKey = 'memory' | 'cpu'

function Processes({ s, users }: { s: NodeSystem; users: System['users'] }) {
  const [sort, setSort] = useState<SortKey>('memory')
  const mem = (p: SysProcess) => p.pss_bytes ?? p.rss_bytes
  const rows = [...s.processes].sort((a, b) => (sort === 'cpu' ? (b.cpu_percent ?? -1) - (a.cpu_percent ?? -1) : mem(b) - mem(a)) || a.pid - b.pid)
  const totalPss = s.processes.reduce((t, p) => t + mem(p), 0)
  const hasPss = s.processes.some((p) => p.pss_bytes != null)
  const totalCpu = s.processes.reduce((t, p) => t + (p.cpu_percent ?? 0), 0)
  const name = (p: SysProcess) => p.plugin?.label ?? p.name
  const sorter = (
    <div role="radiogroup" aria-label="排序" className="inline-flex rounded-lg border border-paper-3 bg-cream p-0.5">
      {(['memory', 'cpu'] as const).map((k) => (
        <button key={k} type="button" role="radio" aria-checked={sort === k} onClick={() => setSort(k)}
          className={cn('h-7 cursor-pointer rounded-md px-2.5 text-[13px] font-semibold transition-colors duration-150',
            sort === k ? 'bg-cobalt text-white' : 'text-ink-2 hover:bg-paper-2 hover:text-ink')}>
          {k === 'memory' ? '按内存' : '按 CPU'}
        </button>
      ))}
    </div>
  )
  return (
    <Section id="sys-procs" title="进程" aside={sorter}>
      {rows.length === 0 ? (
        <p className="rounded-2xl border border-dashed border-paper-3 p-4 text-sm text-ink-2">还没有进程样本。</p>
      ) : (
        <>
          <div className={cn(panel, 'hidden overflow-x-auto md:block')}>
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-paper-3 text-left text-[13px] text-ink-3">
                  <th scope="col" className="px-4 py-2.5 font-semibold">进程</th>
                  <th scope="col" className="px-3 py-2.5 font-semibold">归属</th>
                  <th scope="col" className="px-3 py-2.5 text-right font-semibold">PID</th>
                  <th scope="col" className="px-3 py-2.5 text-right font-semibold" title="按共享页比例分摊后的内存">PSS</th>
                  <th scope="col" className="px-3 py-2.5 text-right font-semibold" title="进程独占的内存">USS</th>
                  <th scope="col" className="px-3 py-2.5 text-right font-semibold" title="常驻内存，含共享页">RSS</th>
                  <th scope="col" className="px-3 py-2.5 text-right font-semibold">CPU</th>
                  <th scope="col" className="px-3 py-2.5 text-right font-semibold">线程</th>
                  <th scope="col" className="px-4 py-2.5 text-right font-semibold">已运行</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((p) => (
                  <tr key={p.pid} className="border-b border-paper-3/70 align-top last:border-b-0 hover:bg-white/70">
                    <th scope="row" className="max-w-64 px-4 py-2.5 text-left font-normal">
                      <span className="block truncate font-semibold text-ink" title={name(p)}>{name(p)}</span>
                      <span className="mt-0.5 flex items-center gap-1.5"><RoleTag role={p.role} />{p.plugin && <span className="text-[12px] text-ink-3">{p.name}</span>}</span>
                    </th>
                    <td className="max-w-52 px-3 py-2.5"><Owner p={p} users={users} /></td>
                    <td className="px-3 py-2.5 text-right font-mono text-[13px] text-ink-2">{p.pid}</td>
                    <td className="px-3 py-2.5 text-right tabular-nums font-semibold text-ink">{fmtBytes(p.pss_bytes)}</td>
                    <td className="px-3 py-2.5 text-right tabular-nums text-ink-2">{fmtBytes(p.uss_bytes)}</td>
                    <td className="px-3 py-2.5 text-right tabular-nums text-ink-2">{fmtBytes(p.rss_bytes)}</td>
                    <td className="px-3 py-2.5 text-right tabular-nums text-ink">{p.cpu_percent == null ? '无数据' : `${p.cpu_percent.toFixed(1)}%`}</td>
                    <td className="px-3 py-2.5 text-right tabular-nums text-ink-2">{p.threads ?? '无数据'}</td>
                    <td className="px-4 py-2.5 text-right tabular-nums text-ink-2">{since(p.started_at)}</td>
                  </tr>
                ))}
              </tbody>
              <tfoot>
                <tr className="border-t border-paper-3 bg-paper-2/50 text-[13px]">
                  <th scope="row" colSpan={3} className="px-4 py-2.5 text-left font-semibold text-ink-2">合计 {rows.length} 个进程</th>
                  <td className="px-3 py-2.5 text-right tabular-nums font-semibold text-ink">{fmtBytes(totalPss)}</td>
                  <td colSpan={2} />
                  <td className="px-3 py-2.5 text-right tabular-nums font-semibold text-ink">{totalCpu.toFixed(1)}%</td>
                  <td colSpan={2} />
                </tr>
              </tfoot>
            </table>
          </div>
          <ul className="grid gap-2 md:hidden">
            {rows.map((p) => (
              <li key={p.pid} className={cn(panel, 'p-3.5 text-sm')}>
                <div className="flex items-start gap-2">
                  <div className="min-w-0 flex-1">
                    <p className="truncate font-semibold text-ink">{name(p)}</p>
                    <p className="mt-0.5 flex flex-wrap items-center gap-x-2 text-[12px] text-ink-3"><RoleTag role={p.role} /><span className="font-mono">PID {p.pid}</span><span>已运行 {since(p.started_at)}</span></p>
                  </div>
                  <span className="text-right tabular-nums">
                    <span className="block font-semibold text-ink">{fmtBytes(mem(p))}</span>
                    <span className="block text-[12px] text-ink-3">{p.cpu_percent == null ? '无数据' : `${p.cpu_percent.toFixed(1)}%`}</span>
                  </span>
                </div>
                {p.plugin && <div className="mt-2 text-[13px]"><Owner p={p} users={users} /></div>}
              </li>
            ))}
          </ul>
          <p className="mt-2 text-[13px] leading-relaxed text-ink-3">
            {hasPss ? 'PSS 按共享页比例分摊，合计接近进程实际占用。' : '此节点只提供 RSS。'}
            {s.container && ` 容器内存 ${fmtBytes(s.container.memory_current_bytes)} 是 cgroup 计数，含文件缓存和内核内存，但不计其他分组先读入的共享文件页，所以和 PSS 合计不一定相等。`}
            {' CPU 以一个逻辑核心为 100%。'}
          </p>
        </>
      )}
    </Section>
  )
}

function Tenants({ s, users }: { s: NodeSystem; users: System['users'] }) {
  return (
    <Section id="sys-tenants" title="租户内存" aside="cgroup 实时值">
      {!s.cgroups_enabled ? (
        <p className="rounded-2xl border border-dashed border-paper-3 p-4 text-sm text-ink-2">节点没有启用 cgroup，插件内存只按采样统计。</p>
      ) : s.tenants.length === 0 ? (
        <p className="rounded-2xl border border-dashed border-paper-3 p-4 text-sm text-ink-2">还没有租户分组。插件第一次启动时创建。</p>
      ) : (
        <ul className={cn(panel, 'divide-y divide-paper-3')}>
          {s.tenants.map((t) => {
            const p = t.memory_max_bytes ? pct(t.memory_current_bytes, t.memory_max_bytes) : null
            return (
              <li key={t.user_id} className="grid gap-1.5 px-4 py-3">
                <div className="flex items-baseline justify-between gap-3">
                  <Link to={`/app/admin/users/${t.user_id}/usage`} className="min-w-0 truncate text-sm font-semibold text-ink underline-offset-2 hover:text-sky hover:underline">
                    {users[t.user_id] ?? t.user_id}
                  </Link>
                  <span className="shrink-0 text-sm tabular-nums text-ink">
                    {fmtBytes(t.memory_current_bytes)}<span className="text-ink-3"> / {t.memory_max_bytes ? fmtBytes(t.memory_max_bytes) : '不限'}</span>
                  </span>
                </div>
                {p != null && <Meter value={p} label={`${users[t.user_id] ?? t.user_id} 内存`} />}
                <p className="flex flex-wrap gap-x-3 text-[12px] tabular-nums text-ink-3">
                  <span>峰值 {fmtBytes(t.memory_peak_bytes)}</span>
                  <span>进程 {t.procs}</span>
                  <span className={t.oom_kills ? 'font-semibold text-bad' : undefined}>OOM {t.oom_kills} 次</span>
                </p>
              </li>
            )
          })}
        </ul>
      )}
    </Section>
  )
}

function Rows({ items }: { items: [string, ReactNode, string?][] }) {
  return (
    <dl className="divide-y divide-paper-3">
      {items.map(([k, v, hint]) => (
        <div key={k} className="flex items-baseline justify-between gap-3 px-4 py-2.5 text-sm">
          <dt className="text-ink-2">{k}{hint && <span className="ml-1.5 text-[12px] text-ink-3">{hint}</span>}</dt>
          <dd className="text-right tabular-nums font-semibold text-ink">{v}</dd>
        </div>
      ))}
    </dl>
  )
}

function Storage({ s }: { s: NodeSystem }) {
  return (
    <Section id="sys-storage" title="节点存储" aside={s.disk?.path}>
      <div className={panel}>
        <Rows items={[
          ['插件代码与数据', fmtBytes(s.storage.tenants_bytes), '每 5 分钟统计'],
          ['uv 依赖缓存', s.storage.uv_cache_bytes == null ? '统计中' : fmtBytes(s.storage.uv_cache_bytes)],
          ['对话记录库', fmtBytes(s.storage.conversations_db_bytes)],
          ['节点状态库', fmtBytes(s.storage.node_db_bytes)],
        ]} />
      </div>
    </Section>
  )
}

const statusWords: Record<string, { label: string; tone?: 'ok' | 'warn' | 'bad' }> = {
  running: { label: '运行', tone: 'ok' },
  connected: { label: '已连接', tone: 'ok' },
  starting: { label: '启动中', tone: 'warn' },
  connecting: { label: '连接中', tone: 'warn' },
  installing: { label: '安装中', tone: 'warn' },
  sleeping: { label: '休眠中' },
  stopped: { label: '停止' },
  disabled: { label: '已关闭' },
  error: { label: '出错', tone: 'bad' },
  over_quota: { label: '超额', tone: 'bad' },
}
const kindLabel: Record<string, string> = { qq_official: 'QQ', weixin_ilink: '微信', loopback: '本地测试' }

function CountLine({ label, counts }: { label: string; counts: SysCounts }) {
  const entries = Object.entries(counts).filter(([k, v]) => k !== 'total' && k !== 'kind' && typeof v === 'number' && v > 0)
  const order = Object.keys(statusWords)
  entries.sort(([a], [b]) => (order.indexOf(a) + 99 * +(order.indexOf(a) < 0)) - (order.indexOf(b) + 99 * +(order.indexOf(b) < 0)))
  return (
    <div className="grid grid-cols-[4.5rem_minmax(0,1fr)] items-baseline gap-3 px-4 py-3 text-sm">
      <span className="text-ink-2">{label}</span>
      <span className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
        <span className="tabular-nums font-semibold text-ink">共 {counts.total ?? 0}</span>
        {entries.map(([k, v]) => {
          const w = statusWords[k]
          return (
            <span key={k} className={cn('tabular-nums', w?.tone === 'bad' ? 'font-semibold text-bad' : w?.tone === 'warn' ? 'text-warn' : 'text-ink-2')}>
              {w?.label ?? k} {v}
            </span>
          )
        })}
      </span>
    </div>
  )
}

function Live({ s }: { s: NodeSystem }) {
  return (
    <Section id="sys-live" title="小鱼与平台" aside="节点实时状态">
      <div className={cn(panel, 'divide-y divide-paper-3')}>
        <CountLine label="小鱼" counts={s.bots} />
        {s.platforms.length === 0 ? (
          <div className="px-4 py-3 text-sm text-ink-3">还没有接入平台。</div>
        ) : (
          s.platforms.map((p) => <CountLine key={p.kind} label={kindLabel[p.kind] ?? p.kind} counts={p} />)
        )}
        <CountLine label="插件" counts={s.plugins} />
      </div>
    </Section>
  )
}

function Logs({ s, users }: { s: NodeSystem; users: System['users'] }) {
  const l = s.logs
  return (
    <Section id="sys-logs" title={`近 ${Math.round(l.window_s / 60)} 分钟日志`} aside="来自每只小鱼的日志缓冲">
      <div className={panel}>
        <div className="flex gap-6 border-b border-paper-3 px-4 py-3">
          <p className="text-sm text-ink-2">错误 <strong className={cn('ml-1 text-lg tabular-nums', l.errors ? 'text-bad' : 'text-ink')}>{n(l.errors)}</strong></p>
          <p className="text-sm text-ink-2">警告 <strong className={cn('ml-1 text-lg tabular-nums', l.warns ? 'text-warn' : 'text-ink')}>{n(l.warns)}</strong></p>
        </div>
        {l.bots.length === 0 ? (
          <p className="px-4 py-3 text-sm text-ink-3">这段时间没有错误或警告。</p>
        ) : (
          <ul className="divide-y divide-paper-3">
            {l.bots.map((b) => (
              <li key={b.bot_id}>
                <Link to={`/app/admin/users/${b.user_id}/logs`} className="block px-4 py-3 transition-colors duration-150 hover:bg-white focus-visible:bg-white">
                  <span className="flex items-baseline justify-between gap-3">
                    <span className="min-w-0 truncate text-sm font-semibold text-ink">{b.bot_name}</span>
                    <span className="shrink-0 text-[13px] tabular-nums">
                      {b.errors > 0 && <span className="font-semibold text-bad">错误 {b.errors}</span>}
                      {b.errors > 0 && b.warns > 0 && <span className="text-ink-3"> · </span>}
                      {b.warns > 0 && <span className="text-warn">警告 {b.warns}</span>}
                    </span>
                  </span>
                  <span className="block truncate text-[12px] text-ink-3">{users[b.user_id] ?? b.user_id}</span>
                  {b.last_error && (
                    <span className="mt-1 block text-[13px] leading-relaxed text-ink-2">
                      <span className="line-clamp-2 break-words">{b.last_error}</span>
                      <span className="text-[12px] tabular-nums text-ink-3">{fmtAgo(b.last_error_at)}</span>
                    </span>
                  )}
                </Link>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Section>
  )
}

const tableLabel: Record<string, string> = {
  users: '用户', sessions: '登录会话', bots: '小鱼', platforms: '平台', plugins: '插件', llm_keys: '模型 Key',
  invite_codes: '邀请码', invite_redemptions: '兑换记录', gateway_accounts: '活动额度账户', gateway_credits: '活动额度', email_codes: '邮件验证码', captchas: '图形验证码',
  rate_events: '限流记录', qr_tasks: '扫码任务', node_tombstones: '待删除记录', nodes: '节点',
}

function Console({ c }: { c: System['console'] }) {
  const d = c.d1
  return (
    <Section id="sys-console" title="控制台与数据库" aside={c.colo ? `本次请求由 Cloudflare ${c.colo} 处理` : undefined}>
      <div className="grid gap-4 lg:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
        <div className={panel}>
          <Rows items={[
            ['D1 大小', d.size_bytes == null ? '无数据' : fmtBytes(d.size_bytes)],
            ['查询耗时', d.query_ms == null ? '无数据' : `${d.query_ms.toFixed(2)} ms`, 'D1 内部'],
            ['查询往返', `${d.round_trip_ms} ms`, 'Worker 到 D1'],
            ['有效登录会话', n(d.sessions_active)],
            ['24 小时内登录', `${n(d.users_login_24h)} 人`],
            ['应当运行的小鱼', n(d.bots_desired_running)],
            ['等待同步到节点', <span className={d.bots_needs_push ? 'text-bad' : undefined}>{n(d.bots_needs_push)}</span>],
            ['进行中的扫码', n(d.qr_tasks_active)],
          ]} />
        </div>
        <div className={cn(panel, 'p-4')}>
          <p className="mb-2 text-sm font-semibold text-ink">表行数</p>
          <dl className="grid grid-cols-2 gap-x-6 sm:grid-cols-3">
            {d.tables.map((t) => (
              <div key={t.name} className="flex items-baseline justify-between gap-2 border-b border-paper-3/70 py-1.5 text-sm">
                <dt className="min-w-0 truncate text-ink-2" title={t.name}>{tableLabel[t.name] ?? t.name}</dt>
                <dd className="tabular-nums font-semibold text-ink">{n(t.rows)}</dd>
              </div>
            ))}
          </dl>
        </div>
      </div>
    </Section>
  )
}

function NodeFacts({ node }: { node: AdminSystemNode }) {
  const s = node.system
  const facts: [string, ReactNode][] = [
    ['响应', `${node.latency_ms} ms`],
    ...(s ? ([
      ['采样', s.sampled_at ? fmtAgo(s.sampled_at) : '尚未采样'],
      ['版本', <span className="font-mono">{s.version}</span>],
      ['节点程序已运行', since(s.started_at)],
      ['宿主机已运行', fmtDuration(s.host?.uptime_s)],
      ...(s.host?.kernel ? [['内核', <span className="font-mono">{s.host.kernel}</span>] as [string, ReactNode]] : []),
    ] as [string, ReactNode][]) : [['最后联系', fmtAgo(node.last_seen)] as [string, ReactNode]]),
  ]
  return (
    <div className="mb-4 flex flex-wrap items-center gap-x-5 gap-y-2">
      <span className="flex items-center gap-2">
        <span className="font-display text-xl font-black text-ink">{node.id}</span>
        {node.online ? <Pill tone="ok">在线</Pill> : <Pill tone="bad">离线</Pill>}
      </span>
      <dl className="flex flex-wrap gap-x-5 gap-y-1 text-[13px]">
        {facts.map(([k, v]) => (
          <div key={k} className="flex gap-1.5">
            <dt className="text-ink-3">{k}</dt>
            <dd className="tabular-nums text-ink">{v}</dd>
          </div>
        ))}
      </dl>
    </div>
  )
}

function NodeView({ node, users }: { node: AdminSystemNode; users: System['users'] }) {
  const s = node.system
  return (
    <section aria-label={`节点 ${node.id}`} className="grid gap-8">
      <div>
        <NodeFacts node={node} />
        {!s ? (
          <div role="alert" className="rounded-2xl border border-bad/25 bg-bad-soft px-4 py-3 text-sm text-bad">
            <p className="font-semibold">节点没有回应，下面的节点数据暂时缺失。</p>
            {node.error && <p className="mt-1">{node.error}</p>}
            <p className="mt-1 break-all font-mono text-[12px]">{node.base_url}</p>
          </div>
        ) : (
          <div className="grid gap-4">
            <Resources s={s} />
            <Trends s={s} />
          </div>
        )}
      </div>
      {s && (
        <>
          <Processes s={s} users={users} />
          <div className="grid gap-8 lg:grid-cols-2">
            <Tenants s={s} users={users} />
            <div className="grid content-start gap-8">
              <Live s={s} />
              <Storage s={s} />
            </div>
          </div>
          <Logs s={s} users={users} />
        </>
      )}
    </section>
  )
}

export default function AdminSystem() {
  const q = useAdminSystem()
  const [picked, setPicked] = useState<string | null>(null)
  const nodes = q.data?.nodes ?? []
  const node = nodes.find((x) => x.id === picked) ?? nodes[0]
  const actions = (
    <>
      {q.data && (
        <span className="text-[13px] tabular-nums text-ink-3" aria-live="polite">
          更新于 {fmtTime(q.data.generated_at)}
        </span>
      )}
      <Button size="sm" onClick={() => q.refetch()} loading={q.isFetching} disabled={q.isLoading}>
        {!q.isFetching && <RefreshCw aria-hidden />}
        刷新
      </Button>
    </>
  )
  return (
    <>
      <PageHeader title="系统监控" description={`节点每 15 秒采样一次，本页每 ${SYSTEM_REFRESH_MS / 1000} 秒自动刷新。`} actions={actions} />
      {q.isLoading ? (
        <div className="grid gap-4">
          <Skeleton className="h-7 w-80" />
          <Skeleton className="h-32" />
          <div className="grid gap-4 lg:grid-cols-2"><Skeleton className="h-56" /><Skeleton className="h-56" /></div>
          <Skeleton className="h-72" />
        </div>
      ) : q.error && !q.data ? (
        <ErrorNote error={q.error} onRetry={() => q.refetch()} />
      ) : q.data ? (
        <div className="grid gap-10">
          {q.error && <ErrorNote error={q.error} onRetry={() => q.refetch()} />}
          {nodes.length > 1 && (
            <div role="tablist" aria-label="节点" className="flex flex-wrap gap-2">
              {nodes.map((x) => (
                <button key={x.id} type="button" role="tab" aria-selected={x.id === node?.id} onClick={() => setPicked(x.id)}
                  className={cn('inline-flex h-9 cursor-pointer items-center gap-2 rounded-xl border px-3 text-sm font-semibold transition-colors duration-150',
                    x.id === node?.id ? 'border-cobalt bg-cobalt text-white' : 'border-paper-3 bg-cream text-ink hover:bg-white')}>
                  <span className={cn('size-1.5 rounded-full', x.online ? 'bg-ok' : 'bg-bad')} aria-hidden />
                  {x.id}
                </button>
              ))}
            </div>
          )}
          {node ? (
            <NodeView node={node} users={q.data.users} />
          ) : (
            <p className="rounded-2xl border border-dashed border-paper-3 p-4 text-[15px] text-ink-2">暂无节点。请在 Worker 中配置节点地址和密钥。</p>
          )}
          <Console c={q.data.console} />
        </div>
      ) : null}
    </>
  )
}
