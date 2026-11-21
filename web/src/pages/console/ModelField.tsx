import { useState } from 'react'
import { Field, Input } from '@/components/ui'
import { Select } from './bot/shared'

export default function ModelField({
  id,
  value,
  onChange,
  models,
  error,
  disabled,
  fallback,
}: {
  id: string
  value: string
  onChange: (model: string) => void
  models?: readonly string[]
  error?: string
  disabled?: boolean
  fallback?: string
}) {
  const [manual, setManual] = useState(false)
  const hasModels = !!models?.length
  const options = [...new Set([...(models ?? []), fallback, value].filter((model): model is string => !!model))]

  return (
    <Field
      label="模型名"
      htmlFor={id}
      error={error}
      aside={
        hasModels && (
          <button
            type="button"
            className="cursor-pointer text-[13px] font-semibold text-sky hover:underline disabled:opacity-55"
            disabled={disabled}
            onClick={() => setManual(!manual)}
          >
            {manual ? '从列表选择' : '手动输入'}
          </button>
        )
      }
    >
      {hasModels && !manual ? (
        <Select id={id} value={value} onChange={(e) => onChange(e.target.value)} aria-invalid={!!error} disabled={disabled}>
          <option value="" disabled={!fallback}>
            {fallback ? '使用 Key 的默认模型' : '选择模型'}
          </option>
          {options.map((model) => (
            <option key={model}>{model}</option>
          ))}
        </Select>
      ) : (
        <Input
          id={id}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={fallback}
          autoCapitalize="off"
          autoComplete="off"
          spellCheck={false}
          aria-invalid={!!error}
          disabled={disabled}
        />
      )}
    </Field>
  )
}
