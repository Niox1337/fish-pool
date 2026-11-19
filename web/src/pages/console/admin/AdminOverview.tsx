import type { ReactNode } from 'react'
import { Link } from 'react-router'
import { ChevronRight, Ticket } from 'lucide-react'
import { useAdminOverview } from '@/api/hooks'
import type { AdminNode, AdminOverview as Overview, AdminProblem } from '@/api/types'
import { ErrorNote, PageHeader, Pill, Skeleton, Stat } from '@/components/ui'
import { FoodBar } from '@/components/FoodBar'
import { Pupil } from '@/components/Pupil'
import { cn } from '@/lib/cn'
import { fmtAgo, fmtBytes, pct } from '@/lib/format'

const MB = 1024 * 1024

function Block({ title, children, aside, className }: { title: string; children: ReactNode; aside?: ReactNode; className?: string }) {
  return (
    <section className={cn('rounded-2xl border border-paper-3 bg-cream p-4 sm:p-5', className)}>
      <div className="mb-3 flex items-baseline justify-between gap-3">
        <h2 className="font-display text-base font-black text-ink">{title}</h2>
        {aside}
      </div>
      {children}
    </section>
  )
}

const n = (v: number) => v.toLocaleString('zh-CN')

function Counts({ o }: { o: Overview }) {
  return (
    <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
      <Block title="用户" aside={<Link to="/app/admin/users" className="text-[13px] font-semibold text-sky">全部用户</Link>}>
        <dl className="grid grid-cols-3 gap-x-3 gap-y-3">
          <Stat label="总数" value={n(o.users.total)} />
          <Stat label="7 天新增" value={n(o.users.new_7d)} />
          <Stat label="服务有效" value={n(o.users.active)} />
          <Stat label="将到期" value={n(o.users.expiring_7d)} sub="7 天内" tone={o.users.expiring_7d ? 'warn' : undefined} />
          <Stat label="已到期" value={n(o.users.expired)} tone={o.users.expired ? 'bad' : undefined} />
        </dl>
      </Block>
      <Block title="邀请码">
        <dl className="grid grid-cols-2 gap-x-3 gap-y-3">
          <Stat label="未使用" value={n(o.invites.unused)} tone={o.invites.unused === 0 ? 'warn' : undefined} />
          <Stat label="7 天内兑换" value={n(o.invites.used_7d)} />
        </dl>
        <Link
          to="/app/admin/invites"
          className="mt-4 inline-flex h-8 items-center gap-1.5 rounded-lg border border-star-2 bg-star px-3 text-[13px] font-semibold text-ink shadow-[inset_0_-2px_0_rgb(160_100_0/0.22)] transition-[filter] duration-150 hover:brightness-[1.04]"
        >
          <Ticket className="size-3.5" aria-hidden />
          发新的邀请码
        </Link>
      </Block>
      <Block title="小鱼">
        <dl className="grid grid-cols-3 gap-x-3 gap-y-3">
          <Stat label="总数" value={n(o.bots.total)} />
          <Stat label="在上课" value={n(o.bots.running)} />
          <Stat label="正在入座" value={n(o.bots.starting)} />
          <Stat label="在睡觉" value={n(o.bots.stopped)} />
          <Stat label="不舒服" value={n(o.bots.error)} tone={o.bots.error ? 'bad' : undefined} />
        </dl>
      </Block>
      <Block title="插件">
        <dl className="grid grid-cols-3 gap-x-3 gap-y-3">
          <Stat label="总数" value={n(o.plugins.total)} />
          <Stat label="运行中" value={n(o.plugins.running)} />
          <Stat label="安装中" value={n(o.plugins.installing)} />
          <Stat label="出错" value={n(o.plugins.error)} tone={o.plugins.error ? 'bad' : undefined} />
          <Stat label="超出配额" value={n(o.plugins.over_quota)} tone={o.plugins.over_quota ? 'bad' : undefined} />
        </dl>
      </Block>
      <Block title="接入平台">
        <dl className="grid grid-cols-3 gap-x-3 gap-y-3">
          <Stat label="QQ" value={n(o.platforms.qq_official)} />
          <Stat label="微信" value={n(o.platforms.weixin_ilink)} />
          <Stat label="出错" value={n(o.platforms.error)} tone={o.platforms.error ? 'bad' : undefined} />
        </dl>
      </Block>
      <Block title="总用量">
        <div className="grid gap-4">
          <FoodBar label="内存" compact capacity={o.usage.quota_memory_mb * MB} servings={[{ key: 'm', label: '全部用户', bytes: o.usage.memory_bytes }]} />
          <FoodBar label="存储" compact capacity={o.usage.quota_storage_mb * MB} servings={[{ key: 's', label: '全部用户', bytes: o.usage.storage_bytes }]} />
          <p className="text-[13px] text-ink-3">分母是所有用户额度之和，不是节点容量。</p>
        </div>
      </Block>
    </div>
  )
}

const kindLabel: Record<AdminProblem['kind'], string> = { bot: '小鱼', platform: '平台', plugin: '插件' }
const statusLabel: Record<string, string> = { error: '出错', over_quota: '超出配额' }

function Problems({ items }: { items: AdminProblem[] }) {
  const sorted = [...items].sort((a, b) => b.updated_at - a.updated_at)
  return (
    <section aria-labelledby="ov-problems">
      <h2 id="ov-problems" className="mb-3 flex items-center gap-2 font-display text-lg font-black text-ink">
        需要看看的
        {sorted.length > 0 && <span className="text-sm font-semibold text-bad tabular-nums">{sorted.length}</span>}
      </h2>
      {sorted.length === 0 ? (
        <div className="flex items-center gap-4 rounded-2xl border border-dashed border-paper-3 p-4 text-[15px] text-ink-2">
          <Pupil mood="happy" size={56} />
          所有小鱼、平台和插件都好好的。出错或超出配额的会按时间列在这里。
        </div>
      ) : (
        <ul className="divide-y divide-paper-3 overflow-hidden rounded-2xl border border-paper-3 bg-cream">
          {sorted.map((p) => (
            <li key={`${p.kind}-${p.id}`}>
              <Link
                to={`/app/admin/users/${p.user_id}`}
                className="group flex items-start gap-3 px-4 py-3 transition-colors duration-150 hover:bg-white focus-visible:bg-white"
              >
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <Pill tone="bad">{statusLabel[p.status] ?? p.status}</Pill>
                    <span className="text-[13px] text-ink-3">{kindLabel[p.kind]}</span>
                    <span className="font-semibold text-ink">{p.label}</span>
                  </div>
                  {p.status_detail && <p className="mt-1 text-sm text-ink-2">{p.status_detail}</p>}
                  <p className="mt-1 flex flex-wrap gap-x-3 text-[13px] text-ink-3">
                    <span className="break-all">{p.user_email}</span>
                    <span>{p.bot_name}</span>
                    <span className="tabular-nums">{fmtAgo(p.updated_at)}</span>
                  </p>
                </div>
                <ChevronRight className="mt-1 size-4 shrink-0 text-ink-3 transition-transform duration-150 group-hover:translate-x-0.5" aria-hidden />
              </Link>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

function MemUse({ node }: { node: AdminNode }) {
  const p = pct(node.memory_used_bytes, node.memory_total_bytes)
  return (
    <div className="grid gap-1">
      <span className="tabular-nums text-ink">
        {fmtBytes(node.memory_used_bytes)} / {fmtBytes(node.memory_total_bytes)}
      </span>
      <span className="h-1.5 w-full max-w-36 overflow-hidden rounded-full bg-paper-2" aria-hidden>
        <span className={cn('block h-full rounded-full', p > 85 ? 'bg-bad' : p > 70 ? 'bg-star-2' : 'bg-cobalt')} style={{ width: `${p}%` }} />
      </span>
    </div>
  )
}

function NodeState({ node }: { node: AdminNode }) {
  return node.online ? <Pill tone="ok">在线</Pill> : <Pill tone="bad">离线</Pill>
}

function Nodes({ nodes }: { nodes: AdminNode[] }) {
  return (
    <section aria-labelledby="ov-nodes">
      <h2 id="ov-nodes" className="mb-3 font-display text-lg font-black text-ink">
        节点
      </h2>
      {nodes.length === 0 ? (
        <p className="rounded-2xl border border-dashed border-paper-3 p-4 text-[15px] text-ink-2">
          还没有配置节点，小鱼没地方住。在 Worker 配置里加上节点地址和密钥后会出现在这里。
        </p>
      ) : (
        <>
          <table className="hidden w-full overflow-hidden rounded-2xl border border-paper-3 bg-cream text-sm md:table">
            <thead>
              <tr className="border-b border-paper-3 text-left text-[13px] text-ink-3">
                <th scope="col" className="px-4 py-2.5 font-semibold">节点</th>
                <th scope="col" className="px-3 py-2.5 font-semibold">状态</th>
                <th scope="col" className="px-3 py-2.5 font-semibold">版本</th>
                <th scope="col" className="px-3 py-2.5 font-semibold">内存</th>
                <th scope="col" className="px-3 py-2.5 text-right font-semibold">小鱼</th>
                <th scope="col" className="px-4 py-2.5 text-right font-semibold">最后联系</th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((nd) => (
                <tr key={nd.id} className="border-b border-paper-3/70 align-top last:border-b-0">
                  <th scope="row" className="px-4 py-3 text-left font-normal">
                    <span className="font-semibold text-ink">{nd.id}</span>
                    <span className="block font-mono text-[12px] text-ink-3">{nd.base_url}</span>
                    {nd.error && <span className="mt-1 block text-[13px] text-bad">{nd.error}</span>}
                  </th>
                  <td className="px-3 py-3"><NodeState node={nd} /></td>
                  <td className="px-3 py-3 font-mono text-[13px] text-ink-2">{nd.version || '未知'}</td>
                  <td className="px-3 py-3"><MemUse node={nd} /></td>
                  <td className="px-3 py-3 text-right tabular-nums text-ink">{nd.bots}</td>
                  <td className="px-4 py-3 text-right text-ink-2 tabular-nums">{fmtAgo(nd.last_seen)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <ul className="grid gap-3 md:hidden">
            {nodes.map((nd) => (
              <li key={nd.id} className="rounded-2xl border border-paper-3 bg-cream p-4 text-sm">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-semibold text-ink">{nd.id}</span>
                  <NodeState node={nd} />
                  <span className="ml-auto font-mono text-[12px] text-ink-3">{nd.version || '未知'}</span>
                </div>
                <p className="mt-0.5 break-all font-mono text-[12px] text-ink-3">{nd.base_url}</p>
                {nd.error && <p className="mt-1 text-[13px] text-bad">{nd.error}</p>}
                <dl className="mt-3 grid grid-cols-[1fr_auto_auto] items-end gap-x-4 text-[13px]">
                  <div>
                    <dt className="text-ink-3">内存</dt>
                    <dd><MemUse node={nd} /></dd>
                  </div>
                  <div>
                    <dt className="text-ink-3">小鱼</dt>
                    <dd className="tabular-nums text-ink">{nd.bots}</dd>
                  </div>
                  <div>
                    <dt className="text-ink-3">最后联系</dt>
                    <dd className="tabular-nums text-ink">{fmtAgo(nd.last_seen)}</dd>
                  </div>
                </dl>
              </li>
            ))}
          </ul>
        </>
      )}
    </section>
  )
}

export default function AdminOverview() {
  const q = useAdminOverview()
  return (
    <>
      <PageHeader title="总览" description="整个托管班的人数、小鱼、插件、节点和需要处理的问题，每 30 秒刷新。" />
      {q.isLoading ? (
        <div className="grid gap-4">
          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {Array.from({ length: 6 }, (_, i) => (
              <Skeleton key={i} className="h-36" />
            ))}
          </div>
          <Skeleton className="h-40" />
        </div>
      ) : q.error ? (
        <ErrorNote error={q.error} onRetry={() => q.refetch()} />
      ) : q.data ? (
        <div className="grid gap-8">
          <Counts o={q.data} />
          <Problems items={q.data.problems} />
          <Nodes nodes={q.data.nodes} />
        </div>
      ) : null}
    </>
  )
}
