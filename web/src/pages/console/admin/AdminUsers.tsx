import { useEffect, useState } from 'react'
import { Link } from 'react-router'
import { ChevronRight, Search } from 'lucide-react'
import { api } from '@/api/client'
import { useAdminUsers } from '@/api/hooks'
import type { AdminUser } from '@/api/types'
import { Button, ErrorNote, Input, PageHeader, Skeleton } from '@/components/ui'
import { cn } from '@/lib/cn'
import { fmtAgo, fmtBytes } from '@/lib/format'
import { ServiceStatus } from './shared'

function useDebounced<T>(value: T, ms: number) {
  const [v, setV] = useState(value)
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms)
    return () => clearTimeout(t)
  }, [value, ms])
  return v
}

function BotsCell({ u }: { u: AdminUser }) {
  if (u.bots === 0) return <span className="text-ink-3">没有小鱼</span>
  return (
    <span className="tabular-nums text-ink">
      {u.bots} 条<span className="text-ink-3">，{u.running} 运行中</span>
      {u.errors > 0 && <span className="font-semibold whitespace-nowrap text-bad">，{u.errors} 出错</span>}
    </span>
  )
}

function UsageCell({ u, labelled }: { u: AdminUser; labelled?: boolean }) {
  if (u.memory_bytes == null) return <span className="text-ink-3">{labelled ? '节点没响应，用量未知' : '节点没响应'}</span>
  const over = u.memory_bytes > u.quota.memory_mb * 1024 * 1024
  return (
    <span className="tabular-nums">
      {labelled && <span className="text-ink-3">内存 </span>}
      <span className={cn('text-ink', over && 'font-semibold text-bad')}>{fmtBytes(u.memory_bytes)}</span>
      <span className="text-ink-3">{labelled ? '，存储 ' : ' / '}{fmtBytes(u.storage_bytes)}</span>
    </span>
  )
}

function RoleTag({ u }: { u: AdminUser }) {
  return u.role === 'admin' ? <span className="text-[12px] font-semibold text-sky">管理员</span> : null
}

export default function AdminUsers() {
  const [input, setInput] = useState('')
  const q = useDebounced(input.trim(), 300)
  const first = useAdminUsers(q)
  const [more, setMore] = useState<{ q: string; items: AdminUser[]; cursor: string | null } | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)
  const [moreError, setMoreError] = useState<unknown>(null)

  const extra = more && more.q === q ? more : null
  const items = [...(first.data?.items ?? []), ...(extra?.items ?? [])]
  const cursor = extra ? extra.cursor : first.data?.cursor ?? null
  const stale = first.isFetching && first.isPlaceholderData

  async function loadMore() {
    if (!cursor) return
    setLoadingMore(true)
    setMoreError(null)
    try {
      const r = await api.adminUsers(q || undefined, cursor)
      setMore({ q, items: [...(extra?.items ?? []), ...r.items], cursor: r.cursor })
    } catch (e) {
      setMoreError(e)
    } finally {
      setLoadingMore(false)
    }
  }

  return (
    <>
      <PageHeader title="用户" description="按邮箱搜索，点进去可以延长服务、调整额度、看用量和日志。" />
      <div className="relative mb-5 max-w-md">
        <Search className="pointer-events-none absolute top-1/2 left-3.5 size-4 -translate-y-1/2 text-ink-3" aria-hidden />
        <Input
          type="search"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="搜索邮箱"
          aria-label="按邮箱搜索用户"
          className="pl-10"
        />
      </div>

      {first.isLoading ? (
        <div className="grid gap-2">
          {Array.from({ length: 5 }, (_, i) => (
            <Skeleton key={i} className="h-14" />
          ))}
        </div>
      ) : first.error ? (
        <ErrorNote error={first.error} onRetry={() => first.refetch()} />
      ) : items.length === 0 ? (
        <p className="rounded-2xl border border-dashed border-paper-3 p-5 text-[15px] text-ink-2">
          {q ? `没有邮箱包含“${q}”的用户，换个关键词试试。` : '还没有用户。发出邀请码，有人注册后会出现在这里。'}
        </p>
      ) : (
        <div className={cn('transition-opacity duration-150', stale && 'opacity-60')} aria-busy={stale || undefined}>
          <table className="hidden w-full table-fixed overflow-hidden rounded-2xl border border-paper-3 bg-cream text-sm lg:table">
            <thead>
              <tr className="border-b border-paper-3 text-left text-[13px] text-ink-3">
                <th scope="col" className="w-[27%] px-4 py-2.5 font-semibold">邮箱</th>
                <th scope="col" className="w-[13%] px-3 py-2.5 font-semibold">服务</th>
                <th scope="col" className="px-3 py-2.5 font-semibold">小鱼</th>
                <th scope="col" className="w-[22%] px-3 py-2.5 font-semibold">内存 / 存储</th>
                <th scope="col" className="w-[14%] px-4 py-2.5 text-right font-semibold">最近登录</th>
              </tr>
            </thead>
            <tbody>
              {items.map((u) => (
                <tr key={u.id} className="group relative border-b border-paper-3/70 transition-colors duration-150 last:border-b-0 hover:bg-white">
                  <th scope="row" className="px-4 py-3 text-left font-normal">
                    <Link
                      to={`/app/admin/users/${u.id}`}
                      className="block truncate font-semibold text-ink after:absolute after:inset-0 focus-visible:outline-none group-has-[:focus-visible]:underline"
                      title={u.email}
                    >
                      {u.email}
                    </Link>
                    <RoleTag u={u} />
                  </th>
                  <td className="px-3 py-3"><ServiceStatus user={u} compact /></td>
                  <td className="px-3 py-3"><BotsCell u={u} /></td>
                  <td className="px-3 py-3 whitespace-nowrap"><UsageCell u={u} /></td>
                  <td className="px-4 py-3 text-right whitespace-nowrap text-ink-2 tabular-nums">{u.last_login_at ? fmtAgo(u.last_login_at) : '从未登录'}</td>
                </tr>
              ))}
            </tbody>
          </table>

          <ul className="divide-y divide-paper-3 overflow-hidden rounded-2xl border border-paper-3 bg-cream lg:hidden">
            {items.map((u) => (
              <li key={u.id}>
                <Link to={`/app/admin/users/${u.id}`} className="flex items-center gap-3 px-4 py-3 transition-colors duration-150 hover:bg-white">
                  <div className="min-w-0 flex-1 text-[13px]">
                    <div className="flex flex-wrap items-center gap-x-2">
                      <span className="min-w-0 truncate text-[15px] font-semibold text-ink">{u.email}</span>
                      <RoleTag u={u} />
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
                      <ServiceStatus user={u} compact />
                      <BotsCell u={u} />
                    </div>
                    <div className="mt-1 flex flex-wrap gap-x-3 text-ink-3">
                      <UsageCell u={u} labelled />
                      <span className="tabular-nums">{u.last_login_at ? `最近登录 ${fmtAgo(u.last_login_at)}` : '从未登录'}</span>
                    </div>
                  </div>
                  <ChevronRight className="size-4 shrink-0 text-ink-3" aria-hidden />
                </Link>
              </li>
            ))}
          </ul>

          <div className="mt-4 flex flex-wrap items-center gap-3">
            <span className="text-[13px] text-ink-3 tabular-nums">已显示 {items.length} 位</span>
            {cursor && (
              <Button size="sm" onClick={loadMore} loading={loadingMore}>
                加载更多
              </Button>
            )}
          </div>
          {moreError != null && (
            <div className="mt-3">
              <ErrorNote error={moreError} onRetry={loadMore} />
            </div>
          )}
        </div>
      )}
    </>
  )
}
