import type { CSSProperties } from 'react'
import { cn } from '@/lib/cn'
import type { BotStatus } from '@/api/types'
import idle from '@/assets/whale/idle.webp'
import eat from '@/assets/whale/eat.webp'
import sleep from '@/assets/whale/sleep.webp'
import wave from '@/assets/whale/wave.webp'
import dizzy from '@/assets/whale/dizzy.webp'
import happy from '@/assets/whale/happy.webp'
import surprised from '@/assets/whale/surprised.webp'
import portrait from '@/assets/whale/portrait.webp'

export type Mood = 'idle' | 'eat' | 'sleep' | 'wave' | 'dizzy' | 'happy' | 'surprised' | 'portrait'

const sheets: Partial<Record<Mood, { src: string; loop: number }>> = {
  idle: { src: idle, loop: 4.8 },
  eat: { src: eat, loop: 3.2 },
  sleep: { src: sleep, loop: 6.4 },
  wave: { src: wave, loop: 3.2 },
}
const stills: Partial<Record<Mood, string>> = { dizzy, happy, surprised, portrait }

export const statusMood: Record<BotStatus, Mood> = {
  running: 'idle',
  starting: 'eat',
  stopped: 'sleep',
  error: 'dizzy',
}

export function Pupil({
  mood,
  size = 72,
  className,
  play = true,
}: {
  mood: Mood
  size?: number
  className?: string
  play?: boolean
}) {
  const sheet = sheets[mood]
  if (sheet) {
    const style = {
      width: size,
      height: size,
      backgroundImage: `url(${sheet.src})`,
      '--pupil-size': `${size}px`,
      '--pupil-loop': `${sheet.loop}s`,
      '--pupil-row': `${sheet.loop / 8}s`,
    } as CSSProperties
    return <span aria-hidden className={cn('pupil-sheet inline-block shrink-0', className)} style={style} data-play={play || undefined} />
  }
  return (
    <img
      aria-hidden
      alt=""
      src={stills[mood] ?? portrait}
      width={size}
      height={size}
      className={cn('inline-block shrink-0 object-contain', className)}
      style={{ width: size, height: size }}
    />
  )
}
