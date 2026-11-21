import { useEffect, useRef, useState, type CSSProperties } from 'react'
import { Plus, Search } from 'lucide-react'

/** Demo plugin names, cycled in order. Any three consecutive entries get three different colours. */
const POOL = ['每日一言', '天气预报', '群聊小游戏', '今日运势', '早安晚安', '表情包斗图']
const TONES = ['pink', 'star', 'lilac'] as const
/** Memory share of each demo plugin in the usage bar, in the same units as CORE, CHAT and TOTAL. */
const MEM: Record<string, number> = { 每日一言: 1.5, 天气预报: 3, 群聊小游戏: 2.5, 今日运势: 2, 早安晚安: 1.5, 表情包斗图: 3.5 }
const CORE = 5
const CHAT = 2
const TOTAL = 22

type Row = { id: number; name: string; ready: boolean; leaving?: boolean; fresh?: boolean }

const START: Row[] = [
  { id: 2, name: '群聊小游戏', ready: false },
  { id: 1, name: '天气预报', ready: true },
  { id: 0, name: '每日一言', ready: true },
]

const tone = (name: string) => TONES[POOL.indexOf(name) % TONES.length]

/**
 * The plugin page sample. The server render and the reduced motion path show a still frame.
 * While the sample is on screen it types a plugin name, presses 安装 and shelves the plugin, then repeats.
 */
export default function PluginDemo() {
  const root = useRef<HTMLDivElement>(null)
  const [typed, setTyped] = useState(START[0].name)
  const [rows, setRows] = useState(START)
  const [pressed, setPressed] = useState(false)
  const [live, setLive] = useState(false)
  const typedRef = useRef(typed)
  useEffect(() => {
    typedRef.current = typed
  }, [typed])
  const next = useRef(3)
  const ids = useRef(3)

  useEffect(() => {
    const el = root.current
    const motion = window.matchMedia('(prefers-reduced-motion: no-preference)')
    if (!el || !('IntersectionObserver' in window)) return
    let onScreen = false
    const update = () => setLive(onScreen && motion.matches && document.visibilityState === 'visible')
    const io = new IntersectionObserver(([e]) => {
      onScreen = e.isIntersecting
      update()
    })
    io.observe(el)
    motion.addEventListener('change', update)
    document.addEventListener('visibilitychange', update)
    return () => {
      io.disconnect()
      motion.removeEventListener('change', update)
      document.removeEventListener('visibilitychange', update)
    }
  }, [])

  useEffect(() => {
    if (!live) return
    let stop = false
    const timers = new Set<number>()
    const wait = (ms: number) =>
      new Promise<void>((resolve) => {
        const t = window.setTimeout(() => {
          timers.delete(t)
          resolve()
        }, ms)
        timers.add(t)
      })

    async function run() {
      while (!stop) {
        await wait(1300)
        if (stop) return
        setRows((r) => r.map((x, i) => (i === 0 ? { ...x, ready: true, fresh: false } : x)))
        await wait(2000)
        for (let n = typedRef.current.length - 1; n >= 0 && !stop; n--) {
          setTyped(typedRef.current.slice(0, n))
          await wait(55)
        }
        if (stop) return
        const name = POOL[next.current % POOL.length]
        next.current += 1
        await wait(350)
        for (let n = 1; n <= name.length && !stop; n++) {
          setTyped(name.slice(0, n))
          await wait(150)
        }
        if (stop) return
        await wait(450)
        setPressed(true)
        await wait(200)
        setPressed(false)
        const id = ids.current++
        setRows((r) => {
          const [a, b, c] = r.filter((x) => !x.leaving)
          return [{ id, name, ready: false, fresh: true }, a, b, { ...c, leaving: true }]
        })
        await wait(700)
        setRows((r) => r.filter((x) => !x.leaving))
      }
    }
    run()
    return () => {
      stop = true
      timers.forEach(clearTimeout)
      setPressed(false)
      setRows((r) => r.filter((x) => !x.leaving))
    }
  }, [live])

  const shelved = rows.filter((r) => !r.leaving)
  const parts = [
    { key: 'core', cls: 'ls-seg-core', mem: CORE, fresh: false },
    { key: 'chat', cls: 'ls-seg-chat', mem: CHAT, fresh: false },
    ...rows.map((r) => ({ key: r.id, cls: `ls-tone-${tone(r.name)}`, mem: r.leaving ? 0 : MEM[r.name], fresh: !!r.fresh })),
  ]
  let used = 0
  const segments = parts.map((p, i) => {
    const from = (used / TOTAL) * 100
    used += p.mem
    return { ...p, from, end: (used / TOTAL) * 100, z: parts.length - i }
  })

  return (
    <div ref={root} className="ls-page ls-page-demo" data-live={live ? '' : undefined}>
      <span className="ls-demo-tag">示例</span>
      <div className="ls-demo" aria-hidden>
        <div className="ls-demo-field">
          <Search className="ls-demo-search" strokeWidth={2.4} />
          <span className="ls-demo-prefix">搜索插件 </span>
          <span className="ls-demo-typed">{typed}</span>
          <span className="ls-demo-caret" />
        </div>
        <span className="ls-demo-add" data-pressed={pressed ? '' : undefined}>
          <Plus strokeWidth={3} />
          安装
        </span>
      </div>
      <ol className="ls-shelf" aria-hidden>
        {rows.map((r, i) => (
          <li
            key={r.id}
            className="ls-shelf-row"
            data-leaving={r.leaving ? '' : undefined}
            data-fresh={r.fresh ? '' : undefined}
            style={{ '--i': i } as CSSProperties}
          >
            <span className={`ls-spine ls-tone-${tone(r.name)}`} />
            <span className="ls-shelf-name">{r.name}</span>
            <span className={r.ready ? 'ls-chip ls-chip-ok' : 'ls-chip ls-chip-warn'}>{r.ready ? '运行中' : '安装中'}</span>
          </li>
        ))}
      </ol>
      <div className="ls-usage" aria-hidden>
        <p className="ls-usage-label">内存用量，按来源</p>
        <div className="ls-usage-bar">
          {segments.map((g) => (
            <span
              key={g.key}
              className={`ls-seg ${g.cls}`}
              data-fresh={g.fresh ? '' : undefined}
              style={{ '--from': g.from, '--end': g.end, zIndex: g.z } as CSSProperties}
            />
          ))}
        </div>
        <ul className="ls-legend">
          <li>
            <i className="ls-seg-core" />
            小鱼本体
          </li>
          <li>
            <i className="ls-seg-chat" />
            聊天记录
          </li>
          {shelved.map((r, i) => (
            <li key={i}>
              <i className={`ls-tone-${tone(r.name)}`} />
              {r.name}
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}
