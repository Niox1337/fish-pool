import type { SVGProps } from 'react'

export function StarBadge({ n, className }: { n: number; className?: string }) {
  return (
    <span className={`relative inline-grid place-items-center ${className ?? ''}`} aria-hidden>
      <svg viewBox="0 0 48 46" className="absolute inset-0 h-full w-full drop-shadow-[0_2px_0_rgb(160_100_0_/_0.35)]">
        <path
          d="M24 3.2c1.6 0 2.9.9 3.6 2.4l4.3 9 9.7 1.4c3.3.5 4.6 4.5 2.2 6.8l-7 6.9 1.7 9.7c.6 3.3-2.9 5.8-5.8 4.2L24 39l-8.7 4.6c-2.9 1.6-6.4-.9-5.8-4.2l1.7-9.7-7-6.9c-2.4-2.3-1.1-6.3 2.2-6.8l9.7-1.4 4.3-9c.7-1.5 2-2.4 3.6-2.4Z"
          fill="var(--color-star)"
          stroke="var(--color-star-2)"
          strokeWidth="1.5"
        />
      </svg>
      <span className="relative pt-[0.12em] font-display font-black leading-none text-white [text-shadow:0_1px_0_rgb(150_90_0_/_0.5)]">
        {n}
      </span>
    </span>
  )
}

export function QqPenguin(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 40 44" fill="currentColor" aria-hidden {...props}>
      <path d="M20 2c7.4 0 12.2 5.6 12.2 13.4 0 1.6-.2 3-.5 4.2 1.9 2.3 4.6 6.2 5.3 10.6.5 3.1-.4 4.6-1.4 4.6-.8 0-1.8-1-2.7-2.6-.6 2.6-2 4.8-3.8 6.4 2.2.8 3.6 1.9 3.6 3.1 0 1.8-3.4 2.3-7 2.3-2.4 0-4.4-.4-5.7-1.1-1.3.7-3.3 1.1-5.7 1.1-3.6 0-7-.5-7-2.3 0-1.2 1.4-2.3 3.6-3.1-1.8-1.6-3.2-3.8-3.8-6.4-.9 1.6-1.9 2.6-2.7 2.6-1 0-1.9-1.5-1.4-4.6.7-4.4 3.4-8.3 5.3-10.6-.3-1.2-.5-2.6-.5-4.2C7.8 7.6 12.6 2 20 2Z" />
    </svg>
  )
}

export function WeChatBubble(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 44 40" fill="currentColor" aria-hidden {...props}>
      <path d="M17 3C8.7 3 2 8.6 2 15.6c0 4 2.2 7.5 5.6 9.8L6.2 30l5.3-2.8c1.7.5 3.6.8 5.5.8h.9a11 11 0 0 1-.4-3c0-7 6.7-12.6 15-12.6h.9C31.9 7.2 25.1 3 17 3Zm-5 7.6a2 2 0 1 1 0 4 2 2 0 0 1 0-4Zm10 0a2 2 0 1 1 0 4 2 2 0 0 1 0-4Z" />
      <path d="M32.5 15C25.6 15 20 19.6 20 25.2s5.6 10.3 12.5 10.3c1.4 0 2.8-.2 4.1-.6l4.2 2.3-1.1-3.8c2.6-1.9 4.3-4.8 4.3-8.2C44 19.6 39.4 15 32.5 15Zm-4.2 5.6a1.7 1.7 0 1 1 0 3.4 1.7 1.7 0 0 1 0-3.4Zm8.4 0a1.7 1.7 0 1 1 0 3.4 1.7 1.7 0 0 1 0-3.4Z" />
    </svg>
  )
}

export function KeySolid(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 44 44" fill="currentColor" aria-hidden {...props}>
      <path d="M29 3a12 12 0 1 1-4.6 23.1L13.5 37H9v4.5c0 .8-.7 1.5-1.5 1.5H3.5C2.7 43 2 42.3 2 41.5v-4.6c0-.4.2-.8.4-1.1L18 20.2A12 12 0 0 1 29 3Zm2.5 6.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Z" />
    </svg>
  )
}
