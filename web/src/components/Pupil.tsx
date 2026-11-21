import { cn } from '@/lib/cn'
import type { Mood } from '@/components/mood'
// Sprite source: YunYueSama, https://github.com/YunYueSama/codex-deepseek-pet. Resized and converted to WebP. Licence in assets/whale-LICENSE.txt.
import idle from '@/assets/whale/idle.webp'
import eat from '@/assets/whale/eat.webp'
import sleep from '@/assets/whale/sleep.webp'
import waveBody from '@/assets/whale/wave-body.webp'
import waveArm from '@/assets/whale/wave-arm.webp?no-inline'
import dizzy from '@/assets/whale/dizzy.webp'
import happy from '@/assets/whale/happy.webp'
import surprised from '@/assets/whale/surprised.webp'
import portrait from '@/assets/whale/portrait.webp'

const sheets: Partial<Record<Mood, string>> = { idle, eat, sleep }
const stills: Partial<Record<Mood, string>> = { dizzy, happy, surprised, portrait }

export function Pupil({ mood, size = 72, className }: { mood: Mood; size?: number; className?: string }) {
  if (mood === 'wave') {
    /** Body and forearm drawn as two layers. The forearm rotates around the elbow. */
    return (
      <span aria-hidden className={cn('pupil-wave inline-block shrink-0', className)} style={{ width: size, height: size }}>
        <span className="pupil-wave-body" style={{ backgroundImage: `url(${waveBody})` }}>
          <span className="pupil-wave-arm" style={{ backgroundImage: `url(${waveArm})` }} />
        </span>
      </span>
    )
  }
  const sheet = sheets[mood]
  if (sheet) {
    return (
      <span
        aria-hidden
        className={cn('pupil-sheet inline-block shrink-0', className)}
        style={{ width: size, height: size, backgroundImage: `url(${sheet})` }}
      />
    )
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
