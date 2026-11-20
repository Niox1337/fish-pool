import { useState } from 'react'
import { Search } from 'lucide-react'
import { toast } from 'sonner'
import { api } from '@/api/client'
import { useBotMutation, usePluginMarket } from '@/api/hooks'
import type { Bot, MarketPlugin } from '@/api/types'
import { Button, Input, Skeleton } from '@/components/ui'
import { Pupil } from '@/components/Pupil'
import { ActionError } from './shared'

const PAGE_SIZE = 12

export function PluginMarket({ bot, full, onInstalled }: { bot: Bot; full: boolean; onInstalled: () => void }) {
  const [search, setSearch] = useState('')
  const [page, setPage] = useState(0)
  const market = usePluginMarket()
  const install = useBotMutation(bot.id, (plugin: MarketPlugin) => api.addPlugin(bot.id, plugin.id))
  const query = search.trim().toLowerCase()
  const items = market.data?.items ?? []
  const isInstalled = (item: MarketPlugin) => bot.plugins.some((plugin) =>
    plugin.repo_url.toLowerCase() === item.repo_url.toLowerCase()
      || (plugin.name === item.name && plugin.author === item.author),
  )
  const featured = query ? undefined : items.find((item) =>
    item.id === 'Niox1337/astrbot_plugin_gengwiki' && !isInstalled(item),
  )
  const filtered = items.filter((item) => item !== featured
    && [item.display_name, item.name, item.author, item.description, item.category, ...item.tags]
      .some((part) => part.toLowerCase().includes(query)))
  const pages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE))
  const currentPage = Math.min(page, pages - 1)
  const visible = filtered.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE)

  function installButton(item: MarketPlugin) {
    const installed = isInstalled(item)
    const conflict = !installed && bot.plugins.some((plugin) => plugin.name === item.name)
    return (
      <Button
        variant={installed || conflict ? 'secondary' : 'primary'}
        size="sm"
        className="min-h-10 shrink-0"
        aria-label={installed ? item.display_name + ' 已安装' : conflict ? item.display_name + ' 有同名插件' : '安装 ' + item.display_name}
        disabled={full || installed || conflict || install.isPending}
        loading={install.isPending && install.variables?.id === item.id}
        onClick={() => install.mutate(item, {
          onSuccess: () => {
            toast.success(item.display_name + ' 正在安装')
            onInstalled()
          },
        })}
      >
        {installed ? '已安装' : conflict ? '同名插件已安装' : '安装'}
      </Button>
    )
  }

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <div className="relative min-w-0 flex-1 basis-56">
          <Search className="pointer-events-none absolute top-1/2 left-3.5 size-4 -translate-y-1/2 text-ink-3" aria-hidden />
          <Input
            type="search"
            aria-label="搜索市场插件"
            placeholder="搜索插件、作者或标签"
            value={search}
            onChange={(event) => { setSearch(event.target.value); setPage(0) }}
            className="h-10 pl-10"
          />
        </div>
      </div>
      {full && <p className="text-sm text-warn">账号插件额度已满。</p>}
      {install.error && <ActionError error={install.error} />}
      {market.isPending ? (
        <Skeleton className="h-40 w-full" aria-label="正在加载插件目录" />
      ) : market.isError && !market.data ? (
        <div role="status" className="flex flex-wrap items-center gap-3 rounded-xl border border-paper-3 bg-cream p-4">
          <p className="flex-1 basis-48 text-sm text-ink-2">暂时读不到插件市场，稍后再试。</p>
          <Button variant="secondary" size="sm" loading={market.isFetching} onClick={() => void market.refetch()}>重试</Button>
        </div>
      ) : (
        <>
          {featured && (
            <aside aria-label="梗Wiki 插件推荐" className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border border-paper-3 bg-cream px-4 py-3">
              <Pupil mood="happy" size={44} />
              <div className="min-w-0 flex-1 basis-36">
                <p className="font-semibold text-ink">梗Wiki</p>
                <p className="mt-0.5 text-sm text-ink-2">群友抛梗，小鱼接梗，顺手甩张表情包。</p>
              </div>
              {installButton(featured)}
            </aside>
          )}
          {visible.length > 0 ? (
            <ul aria-label="市场插件" className="divide-y divide-paper-3 rounded-2xl border border-paper-3 bg-cream">
              {visible.map((item) => (
                <li key={item.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3 sm:flex-nowrap sm:px-5">
                  <div className="min-w-0 flex-1 basis-44">
                    <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
                      <h3 className="min-w-0 break-words font-semibold text-ink">{item.display_name}</h3>
                      <span className="min-w-0 break-words text-[13px] text-ink-3">
                        {item.author}{item.category && item.category !== '其他' ? ' · ' + item.category : ''}
                      </span>
                    </div>
                    {item.description && <p className="mt-1 break-words text-sm leading-relaxed text-ink-2">{item.description}</p>}
                  </div>
                  <div className="ml-auto flex shrink-0 items-center gap-2">
                    {installButton(item)}
                  </div>
                </li>
              ))}
            </ul>
          ) : !featured && <p role="status" className="py-4 text-sm text-ink-2">没有匹配的插件，换个关键词试试。</p>}
          {pages > 1 && (
            <nav aria-label="市场分页" className="flex flex-wrap items-center justify-between gap-2">
              <p aria-live="polite" className="text-sm text-ink-2 tabular-nums">第 {currentPage + 1} / {pages} 页</p>
              <div className="flex gap-2">
                <Button variant="secondary" size="sm" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}>上一页</Button>
                <Button variant="secondary" size="sm" disabled={currentPage + 1 === pages} onClick={() => setPage(currentPage + 1)}>下一页</Button>
              </div>
            </nav>
          )}
        </>
      )}
    </div>
  )
}
