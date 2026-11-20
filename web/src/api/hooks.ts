import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { api, ApiError } from './client'
import type { Bot } from './types'

export const keys = {
  me: ['me'] as const,
  bots: ['bots'] as const,
  bot: (id: string) => ['bots', id] as const,
  llmKeys: ['llm-keys'] as const,
  gateway: ['gateway'] as const,
  usage: ['usage'] as const,
  logs: (id: string) => ['logs', id] as const,
  admin: ['admin'] as const,
}

export function useMe() {
  return useQuery({
    queryKey: keys.me,
    queryFn: async () => {
      try {
        return (await api.me()).user
      } catch (e) {
        if (e instanceof ApiError && e.status === 401) return null
        throw e
      }
    },
    staleTime: 60_000,
  })
}

export function useBots() {
  return useQuery({ queryKey: keys.bots, queryFn: async () => (await api.bots()).items })
}

export function useBot(id: string) {
  const qc = useQueryClient()
  return useQuery({
    queryKey: keys.bot(id),
    queryFn: () => api.bot(id),
    initialData: () => qc.getQueryData<Bot[]>(keys.bots)?.find((b) => b.id === id),
    refetchInterval: (q) => {
      const b = q.state.data
      const busy = b && (b.status === 'starting' || b.plugins.some((p) => p.status === 'installing'))
      return busy ? 2000 : 15000
    },
  })
}

export function useLlmKeys() {
  return useQuery({ queryKey: keys.llmKeys, queryFn: async () => (await api.llmKeys()).items })
}

export function useKeyBalance(id: string) {
  return useQuery({ queryKey: [...keys.llmKeys, id, 'balance'], queryFn: () => api.llmKeyBalance(id), staleTime: 60_000, retry: false })
}

export function useGateway() {
  return useQuery({ queryKey: keys.gateway, queryFn: api.gateway, staleTime: 60_000, retry: false })
}

export function useUsage() {
  return useQuery({ queryKey: keys.usage, queryFn: api.usage, refetchInterval: 30_000 })
}

export function useLogs(id: string) {
  return useQuery({ queryKey: keys.logs(id), queryFn: () => api.logs(id), refetchInterval: 4000 })
}

export function useBotMutation<TArgs>(botId: string, fn: (args: TArgs) => Promise<unknown>) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSettled: () => {
      qc.invalidateQueries({ queryKey: keys.bot(botId) })
      qc.invalidateQueries({ queryKey: keys.bots })
      qc.invalidateQueries({ queryKey: keys.usage })
    },
  })
}

export function usePluginMarket() {
  return useQuery({ queryKey: ['plugin-market'], queryFn: api.pluginMarket, staleTime: 300_000, retry: false })
}

export function useAdminOverview() {
  return useQuery({ queryKey: [...keys.admin, 'overview'], queryFn: api.adminOverview, refetchInterval: 30_000 })
}

export const SYSTEM_REFRESH_MS = 15_000

export function useAdminSystem() {
  return useQuery({ queryKey: [...keys.admin, 'system'], queryFn: api.adminSystem, refetchInterval: SYSTEM_REFRESH_MS })
}

export function useAdminUsers(q: string) {
  return useQuery({ queryKey: [...keys.admin, 'users', q], queryFn: () => api.adminUsers(q || undefined), placeholderData: (p) => p })
}

export function useAdminUser(id: string) {
  return useQuery({ queryKey: [...keys.admin, 'user', id], queryFn: () => api.adminUser(id), refetchInterval: 15_000 })
}

export function useAdminCredits(id: string) {
  return useQuery({ queryKey: [...keys.admin, 'credits', id], queryFn: () => api.adminCredits(id), retry: false })
}

export function useAdminUserUsage(id: string) {
  return useQuery({ queryKey: [...keys.admin, 'usage', id], queryFn: () => api.adminUserUsage(id), refetchInterval: 30_000 })
}

export function useAdminBotLogs(uid: string, bid: string | null) {
  return useQuery({
    queryKey: [...keys.admin, 'logs', uid, bid],
    queryFn: () => api.adminBotLogs(uid, bid!),
    enabled: !!bid,
    refetchInterval: 4000,
  })
}

export function useAdminInvites(status: 'unused' | 'used' | 'revoked' | 'all') {
  return useQuery({ queryKey: [...keys.admin, 'invites', status], queryFn: () => api.adminInvites(status) })
}
