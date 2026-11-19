import { useState } from 'react'
import type { PluginSchema, SchemaField } from '@/api/types'
import { Field, Input, Switch, Textarea, useFieldId } from '@/components/ui'
import { cn } from '@/lib/cn'
import { ChipsInput, RangeField, Select } from './shared'
import type { Obj } from './schema'

const visible = (f: SchemaField, value: Obj) =>
  !f.invisible && (!f.condition || Object.entries(f.condition).every(([k, v]) => value[k] === v))

function JsonField({
  id,
  value,
  onChange,
  onValidity,
}: {
  id: string
  value: unknown
  onChange: (v: unknown) => void
  onValidity: (ok: boolean) => void
}) {
  const [text, setText] = useState(() => JSON.stringify(value ?? null, null, 2))
  const [bad, setBad] = useState(false)
  return (
    <>
      <Textarea
        id={id}
        value={text}
        spellCheck={false}
        aria-invalid={bad || undefined}
        className="min-h-36 font-mono text-[13px]"
        onChange={(e) => {
          setText(e.target.value)
          try {
            onChange(JSON.parse(e.target.value))
            setBad(false)
            onValidity(true)
          } catch {
            setBad(true)
            onValidity(false)
          }
        }}
      />
      {bad && (
        <p role="alert" className="text-[13px] text-bad">
          JSON 格式不对，检查一下括号和引号
        </p>
      )}
    </>
  )
}

function FieldInput({
  name,
  field,
  value,
  onChange,
  onValidity,
}: {
  name: string
  field: SchemaField
  value: unknown
  onChange: (v: unknown) => void
  onValidity: (key: string, ok: boolean) => void
}) {
  const id = useFieldId(`cfg-${name}`)
  const label = field.description || name
  const hint = field.hint ? (
    <span className={cn(field.obvious_hint && 'font-semibold text-warn')}>{field.hint}</span>
  ) : undefined

  if (field.type === 'bool') {
    return (
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <label htmlFor={id} className="text-sm font-semibold text-ink">
            {label}
          </label>
          {hint && <p className="mt-0.5 text-[13px] leading-relaxed text-ink-3">{hint}</p>}
        </div>
        <Switch id={id} checked={!!value} onCheckedChange={onChange} />
      </div>
    )
  }

  if (field.type === 'object') {
    return (
      <fieldset className="grid gap-4 rounded-xl border border-paper-3 bg-cream p-4">
        <legend className="px-1 text-sm font-semibold text-ink">{label}</legend>
        {hint && <p className="-mt-2 text-[13px] leading-relaxed text-ink-3">{hint}</p>}
        <SchemaForm
          schema={field.items ?? {}}
          value={(value as Obj) ?? {}}
          onChange={onChange}
          onValidity={(k, ok) => onValidity(`${name}.${k}`, ok)}
          prefix={name}
        />
      </fieldset>
    )
  }

  if ((field.type === 'int' || field.type === 'float') && field.slider) {
    const { min, max, step = field.type === 'int' ? 1 : 0.1 } = field.slider
    const decimals = String(step).split('.')[1]?.length ?? 0
    return (
      <RangeField
        id={id}
        label={label}
        hint={hint}
        value={Number(value ?? min)}
        min={min}
        max={max}
        step={step}
        format={(v) => v.toFixed(decimals)}
        onChange={onChange}
      />
    )
  }

  let control
  if (field.options?.length) {
    control = (
      <Select
        id={id}
        value={String(value ?? '')}
        onChange={(e) => {
          const opt = field.options!.find((o) => String(o) === e.target.value)
          onChange(opt ?? e.target.value)
        }}
      >
        {field.options.map((o, i) => (
          <option key={String(o)} value={String(o)}>
            {field.labels?.[i] ?? String(o)}
          </option>
        ))}
      </Select>
    )
  } else if (field.type === 'int' || field.type === 'float') {
    control = (
      <Input
        id={id}
        type="number"
        inputMode={field.type === 'int' ? 'numeric' : 'decimal'}
        step={field.type === 'int' ? 1 : 'any'}
        value={typeof value === 'number' && Number.isFinite(value) ? value : ''}
        onChange={(e) => {
          const n = e.target.value === '' ? NaN : Number(e.target.value)
          const ok = Number.isFinite(n) && (field.type === 'float' || Number.isInteger(n))
          onValidity(name, ok)
          onChange(n)
        }}
        className="tabular-nums sm:max-w-56"
      />
    )
  } else if (field.type === 'list') {
    control = (
      <ChipsInput
        id={id}
        label={label}
        value={Array.isArray(value) ? value.map(String) : []}
        onChange={onChange}
        placeholder="输入后按回车添加"
      />
    )
  } else if (field.type === 'text' || (field.type === 'string' && field.editor_mode)) {
    control = (
      <Textarea
        id={id}
        value={String(value ?? '')}
        onChange={(e) => onChange(e.target.value)}
        className={cn(field.editor_mode && 'font-mono text-[13px]')}
      />
    )
  } else if (field.type === 'dict' || field.type === 'template_list') {
    return (
      <Field label={label} htmlFor={id} hint={hint ?? '这一项用 JSON 编辑。'}>
        <JsonField id={id} value={value} onChange={onChange} onValidity={(ok) => onValidity(name, ok)} />
      </Field>
    )
  } else if (field.type === 'file') {
    return (
      <Field label={label} hint="文件类型的设置暂时不能在网页里上传，当前值如下。">
        <pre className="overflow-x-auto rounded-xl border border-paper-3 bg-paper-2 px-3.5 py-2.5 font-mono text-[13px] text-ink-2">
          {JSON.stringify(value ?? null, null, 2)}
        </pre>
      </Field>
    )
  } else {
    control = <Input id={id} value={String(value ?? '')} onChange={(e) => onChange(e.target.value)} />
  }

  return (
    <Field label={label} htmlFor={id} hint={hint}>
      {control}
    </Field>
  )
}

export function SchemaForm({
  schema,
  value,
  onChange,
  onValidity,
  prefix = '',
}: {
  schema: PluginSchema
  value: Obj
  onChange: (v: Obj) => void
  onValidity: (key: string, ok: boolean) => void
  prefix?: string
}) {
  const entries = Object.entries(schema).filter(([, f]) => visible(f, value))
  if (entries.length === 0) return <p className="text-sm text-ink-3">这个插件没有可以改的设置。</p>
  return (
    <div className="grid gap-5">
      {entries.map(([k, f]) => (
        <FieldInput
          key={`${prefix}.${k}`}
          name={k}
          field={f}
          value={value[k]}
          onChange={(v) => onChange({ ...value, [k]: v })}
          onValidity={onValidity}
        />
      ))}
    </div>
  )
}
