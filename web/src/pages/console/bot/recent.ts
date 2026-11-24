import { createContext, useContext } from 'react'
import type { Bot } from '@/api/types'

/** What a list setting holds: session origins of any type, only private or group chats, bare group ids, or sender account ids. */
export type RecentKind = 'session' | 'friend_session' | 'group_session' | 'group' | 'user'

export const RecentBotContext = createContext<Bot | null>(null)

/** The bot whose recent chats list fields may offer, or null outside a plugin settings form. */
export function useRecentBot() {
  return useContext(RecentBotContext)
}

/** Guesses which recent entries fit a plugin list setting from its key, description and hint. */
export function recentKindOf(name: string, description?: string, hint?: string): RecentKind | null {
  const text = `${name} ${description ?? ''} ${hint ?? ''}`
  if (/umo|unified_msg_origin|session|会话/i.test(text)) {
    const friend = /私聊|好友|friend/i.test(text)
    const group = /群|group/i.test(text)
    return friend === group ? 'session' : friend ? 'friend_session' : 'group_session'
  }
  if (/群号|group_?ids?\b|group_?list|群聊列表/i.test(text)) return 'group'
  if (/admin|管理员|白名单|黑名单|whitelist|blacklist|allow_?list|block_?list|user_?ids?\b|用户\s*id|账号|qq\s*号/i.test(text))
    return 'user'
  return null
}
