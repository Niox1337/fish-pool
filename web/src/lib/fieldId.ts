import { useId } from 'react'

/** A unique element id that starts with `prefix` and contains no colons. */
export function useFieldId(prefix: string) {
  return `${prefix}-${useId().replace(/:/g, '')}`
}
