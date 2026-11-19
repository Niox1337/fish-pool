import { useBots, useMe, useUsage } from '@/api/hooks'
import { ErrorNote, PageHeader } from '@/components/ui'
import { UsageSkeleton, UsageView } from './usage/UsageView'

export default function UsagePage() {
  const { data: me } = useMe()
  const usage = useUsage()
  const bots = useBots()

  return (
    <>
      <PageHeader
        title="资源用量"
        description="每条小鱼本体、聊天记录和每个插件各占一份饭量，都算在你的额度里。数据每 30 秒刷新一次。"
      />
      {usage.isLoading ? (
        <UsageSkeleton />
      ) : usage.error ? (
        <ErrorNote error={usage.error} onRetry={() => usage.refetch()} />
      ) : usage.data ? (
        <UsageView usage={usage.data} bots={bots.data} quota={me?.quota} />
      ) : null}
    </>
  )
}
