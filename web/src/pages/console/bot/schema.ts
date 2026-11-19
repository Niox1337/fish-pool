import type { PluginSchema, SchemaField } from '@/api/types'

export type Obj = Record<string, unknown>

function typeDefault(f: SchemaField): unknown {
  if (f.default !== undefined) return f.default
  switch (f.type) {
    case 'bool':
      return false
    case 'int':
    case 'float':
      return f.slider?.min ?? 0
    case 'list':
    case 'template_list':
      return []
    case 'object':
      return withDefaults(f.items ?? {}, {})
    case 'dict':
      return {}
    default:
      return ''
  }
}

export function withDefaults(schema: PluginSchema, value: Obj): Obj {
  const out: Obj = { ...value }
  for (const [k, f] of Object.entries(schema)) {
    if (out[k] === undefined) out[k] = typeDefault(f)
    else if (f.type === 'object' && f.items && out[k] && typeof out[k] === 'object') out[k] = withDefaults(f.items, out[k] as Obj)
  }
  return out
}
