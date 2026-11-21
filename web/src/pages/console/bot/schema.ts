import type { PluginSchema, SchemaField } from '@/api/types'

export type Obj = Record<string, unknown>

export function isObject(value: unknown): value is Obj {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

export function schemaFields(schema: PluginSchema) {
  return Object.entries(schema).filter(([, field]) => isObject(field))
}

function typeDefault(f: SchemaField): unknown {
  if (f.default !== undefined) return f.default
  switch (f.type) {
    case 'bool':
      return false
    case 'int':
    case 'float':
      return 0
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
  return {
    ...value,
    ...Object.fromEntries(
      schemaFields(schema).map(([name, field]) => {
        const stored = Object.hasOwn(value, name) ? value[name] : typeDefault(field)
        return [name, field.type === 'object' && isObject(stored) ? withDefaults(field.items ?? {}, stored) : stored]
      }),
    ),
  }
}
