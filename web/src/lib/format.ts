const DAY = 86_400_000

export function fmtBytes(n: number | null | undefined): string {
  if (n == null) return '无数据'
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`
}

export function fmtMb(mb: number): string {
  return mb >= 1024 ? `${(mb / 1024).toFixed(mb % 1024 ? 1 : 0)} GB` : `${mb} MB`
}

export function fmtDate(ms: number | null | undefined): string {
  if (!ms) return '无'
  const d = new Date(ms)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

export function fmtDateTime(ms: number | null | undefined): string {
  if (!ms) return '无'
  const d = new Date(ms)
  return `${fmtDate(ms)} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

export function fmtTime(ms: number): string {
  const d = new Date(ms)
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((x) => String(x).padStart(2, '0')).join(':')
}

export function fmtAgo(ms: number | null | undefined): string {
  if (!ms) return '从未'
  const s = Math.round((Date.now() - ms) / 1000)
  if (s < 60) return '刚刚'
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`
  if (s < 30 * 86400) return `${Math.floor(s / 86400)} 天前`
  return fmtDate(ms)
}

export function daysLeft(until: number | null | undefined): number | null {
  if (!until) return null
  return Math.ceil((until - Date.now()) / DAY)
}

export function pct(part: number, whole: number): number {
  if (!whole) return 0
  return Math.min(100, Math.max(0, (part / whole) * 100))
}
