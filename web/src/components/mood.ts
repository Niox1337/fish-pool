import type { BotStatus } from '@/api/types'

export type Mood = 'idle' | 'eat' | 'sleep' | 'wave' | 'dizzy' | 'happy' | 'surprised' | 'portrait'

export const statusMood: Record<BotStatus, Mood> = {
  running: 'idle',
  starting: 'eat',
  stopped: 'sleep',
  error: 'dizzy',
}
