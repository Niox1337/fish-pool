import type { Quota } from '@/api/types'

export const quotaFields: { key: keyof Quota; label: string; unit: string; min: number; max: number }[] = [
  { key: 'memory_mb', label: '内存', unit: 'MiB', min: 64, max: 65536 },
  { key: 'storage_mb', label: '存储', unit: 'MiB', min: 64, max: 1048576 },
  { key: 'bots', label: '小鱼', unit: '只', min: 0, max: 100 },
  { key: 'plugins', label: '插件', unit: '个', min: 0, max: 500 },
]

export type QuotaForm = Record<keyof Quota, string | null>

export function quotaToForm(quota: Quota) {
  return Object.fromEntries(quotaFields.map(({ key }) => [key, quota[key] == null ? null : String(quota[key])])) as QuotaForm
}

export function quotaFromForm(form: QuotaForm) {
  return Object.fromEntries(quotaFields.map(({ key }) => [key, form[key] == null ? null : Number(form[key])])) as Quota
}

export function quotaFieldError(raw: string | null, { min, max }: { min: number; max: number }) {
  if (raw === null && min === 0) return undefined
  const value = Number(raw)
  return raw?.trim() && Number.isInteger(value) && value >= min && value <= max ? undefined : `填写 ${min} 到 ${max} 的整数`
}
