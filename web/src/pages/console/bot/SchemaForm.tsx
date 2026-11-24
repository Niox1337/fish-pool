import { useEffect, useState, type ReactNode } from 'react'
import { ChevronDown, LockKeyhole, Plus, Trash2 } from 'lucide-react'
import type { PluginSchema, SchemaField } from '@/api/types'
import { Button, Field, Input, Switch, Textarea } from '@/components/ui'
import { useFieldId } from '@/lib/fieldId'
import { cn } from '@/lib/cn'
import { recentKindOf, useRecentBot } from './recent'
import { RecentPicker } from './RecentPicker'
import { RangeField, Select } from './shared'
import { isObject, schemaFields, withDefaults, type Obj } from './schema'

type SetValidity = (id: string, valid: boolean) => void

function DraftField({
  id,
  value,
  onChange,
  onValidity,
  kind,
  label,
  hint,
}: {
  id: string
  value: unknown
  onChange: (value: unknown) => void
  onValidity: SetValidity
  kind: 'int' | 'float' | 'dict' | 'list' | 'template_list'
  label: string
  hint?: ReactNode
}) {
  const numeric = kind === 'int' || kind === 'float'
  const [text, setText] = useState(() => (numeric ? String(value ?? '') : JSON.stringify(value ?? null, null, 2)))
  const [error, setError] = useState('')
  useEffect(() => () => onValidity(id, true), [id, onValidity])

  function change(next: string) {
    setText(next)
    let result: unknown
    let message = ''
    if (numeric) {
      const number = next.trim() ? Number(next) : NaN
      if (!Number.isFinite(number) || (kind === 'int' && !Number.isInteger(number))) {
        message = kind === 'int' ? '请输入整数。' : '请输入数字。'
      }
      result = number
    } else {
      try {
        result = JSON.parse(next)
        if (kind === 'dict' ? !isObject(result) : !Array.isArray(result)) {
          message = kind === 'dict' ? '请输入 JSON 对象。' : '请输入 JSON 数组。'
        }
      } catch {
        message = 'JSON 格式有误，请检查括号和引号。'
      }
    }
    setError(message)
    onValidity(id, !message)
    if (!message) onChange(result)
  }

  return (
    <Field label={label} htmlFor={id} hint={hint ?? (numeric ? undefined : '使用 JSON 编辑。')} error={error || undefined}>
      {numeric ? (
        <Input
          id={id}
          inputMode={kind === 'int' ? 'numeric' : 'decimal'}
          value={text}
          aria-invalid={!!error || undefined}
          onChange={(event) => change(event.target.value)}
          className="tabular-nums sm:max-w-52"
        />
      ) : (
        <Textarea
          id={id}
          value={text}
          spellCheck={false}
          aria-invalid={!!error || undefined}
          onChange={(event) => change(event.target.value)}
          className="font-mono text-[13px]"
        />
      )}
    </Field>
  )
}

function ListField({
  name,
  label,
  hint,
  field,
  values,
  onChange,
}: {
  name: string
  label: string
  hint?: ReactNode
  field: SchemaField
  values: unknown[]
  onChange: (values: unknown[]) => void
}) {
  const options = field.options?.length && !field.secret ? field.options : undefined
  const bot = useRecentBot()
  const recentKind = !options && !field.secret ? recentKindOf(name, field.description, field.hint) : null

  function addRecent(value: string) {
    const blank = values.indexOf('')
    onChange(blank < 0 ? [...values, value] : values.map((item, i) => (i === blank ? value : item)))
  }

  return (
    <fieldset className="min-w-0">
      <legend className="text-sm font-semibold text-ink">{label}</legend>
      {hint && <p className="mt-1 text-[13px] leading-relaxed text-ink-3">{hint}</p>}
      <div className="mt-2 grid gap-2">
        {options ? (
          <>
            {options.map((option, index) => (
              <label key={index} className="flex min-h-9 cursor-pointer items-center gap-2.5 rounded-lg text-sm text-ink">
                <input
                  type="checkbox"
                  checked={values.includes(option)}
                  onChange={(event) => onChange(event.target.checked ? [...values, option] : values.filter((value) => value !== option))}
                  className="size-4 shrink-0 accent-cobalt"
                />
                {field.labels?.[index] ?? option}
              </label>
            ))}
          </>
        ) : (
          <>
            {values.map((value, index) => (
              <div key={index} className="flex items-center gap-2">
                <Input
                  type={field.secret ? 'password' : 'text'}
                  value={typeof value === 'string' ? value : ''}
                  placeholder={typeof value === 'string' ? undefined : '此项不是文本'}
                  disabled={typeof value !== 'string'}
                  spellCheck={!field.secret}
                  autoComplete={field.secret ? 'off' : undefined}
                  aria-label={`${label} 第 ${index + 1} 项`}
                  onChange={(event) => onChange(values.map((item, i) => (i === index ? event.target.value : item)))}
                  className="min-w-0 flex-1"
                />
                <Button
                  variant="ghost"
                  size="sm"
                  className="size-10 px-0"
                  aria-label={`移除${label}第 ${index + 1} 项`}
                  onClick={() => onChange(values.filter((_, i) => i !== index))}
                >
                  <Trash2 aria-hidden />
                </Button>
              </div>
            ))}
            <Button variant="secondary" size="sm" className="justify-self-start" onClick={() => onChange([...values, ''])}>
              <Plus aria-hidden />
              添加一项
            </Button>
            {bot && recentKind && <RecentPicker bot={bot} kind={recentKind} values={values} onAdd={addRecent} />}
          </>
        )}
      </div>
    </fieldset>
  )
}

function TemplateList({
  id,
  label,
  hint,
  templates,
  values,
  onChange,
  onValidity,
}: {
  id: string
  label: string
  hint?: ReactNode
  templates: NonNullable<SchemaField['templates']>
  values: unknown[]
  onChange: (values: unknown[]) => void
  onValidity: SetValidity
}) {
  const types = Object.entries(templates).filter(([, template]) => isObject(template))
  const [selected, setSelected] = useState(types[0]?.[0] ?? '')
  const [rows, setRows] = useState(() => values.map(() => ({ id: crypto.randomUUID(), expanded: false })))

  function add() {
    const template = types.find(([name]) => name === selected)?.[1]
    if (!template) return
    setRows([...rows, { id: crypto.randomUUID(), expanded: true }])
    onChange([...values, { ...withDefaults(template.items ?? {}, {}), __template_key: selected }])
  }

  return (
    <fieldset className="min-w-0">
      <legend className="text-sm font-semibold text-ink">{label}</legend>
      {hint && <p className="mt-1 text-[13px] leading-relaxed text-ink-3">{hint}</p>}
      <div className="mt-2 grid gap-2">
        {values.map((value, index) => {
          const row = rows[index]
          const templateKey = isObject(value) ? value.__template_key : undefined
          const template = types.find(([name]) => name === templateKey)?.[1]
          const items = template?.items ?? {}
          const instance = isObject(value) ? value : {}
          const displayField = template?.display_item ? items[template.display_item] : undefined
          const displayValue = template?.display_item ? instance[template.display_item] : undefined
          const detail = displayField && !displayField.secret && !displayField.invisible && typeof displayValue === 'string' && displayValue
          const title = template?.name || (typeof templateKey === 'string' ? templateKey : '旧版配置')
          return (
            <details key={row.id} open={row.expanded} className="group rounded-xl border border-paper-3 bg-cream">
              <summary className="flex cursor-pointer list-none items-center gap-2 rounded-xl px-3.5 py-3 text-sm font-semibold text-ink [&::-webkit-details-marker]:hidden">
                <ChevronDown className="size-4 shrink-0 -rotate-90 text-ink-3 transition-transform group-open:rotate-0" aria-hidden />
                <span className="min-w-0 flex-1 truncate">
                  {title}
                  {detail && <span className="ml-2 font-normal text-ink-3">{detail}</span>}
                </span>
              </summary>
              <div className="grid gap-3 border-t border-paper-3 px-3.5 py-4">
                {template ? (
                  <>
                    {template.hint && <p className="text-[13px] text-ink-3">{template.hint}</p>}
                    <SchemaForm
                      schema={items}
                      value={withDefaults(items, instance)}
                      onChange={(next) =>
                        onChange(values.map((entry, i) => (i === index ? { ...next, __template_key: templateKey } : entry)))
                      }
                      onValidity={onValidity}
                    />
                  </>
                ) : (
                  <p className="text-sm text-ink-3">插件当前版本没有提供这项设置的模板。</p>
                )}
                <Button
                  variant="ghost"
                  size="sm"
                  className="justify-self-start text-bad"
                  onClick={() => {
                    setRows(rows.filter((_, i) => i !== index))
                    onChange(values.filter((_, i) => i !== index))
                  }}
                >
                  <Trash2 aria-hidden />
                  移除这项
                </Button>
              </div>
            </details>
          )
        })}
        {types.length > 0 && (
          <div className="flex flex-wrap items-center gap-2">
            <Select
              id={id}
              aria-label={`${label}类型`}
              value={selected}
              onChange={(event) => setSelected(event.target.value)}
              className="min-w-0 flex-1 basis-36"
            >
              {types.map(([name, template]) => (
                <option key={name} value={name}>
                  {template.name || name}
                </option>
              ))}
            </Select>
            <Button variant="secondary" onClick={add}>
              <Plus aria-hidden />
              添加
            </Button>
          </div>
        )}
      </div>
    </fieldset>
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
  onChange: (value: unknown) => void
  onValidity: SetValidity
}) {
  const id = useFieldId('plugin-setting')
  const label = field.description || name
  const hint = field.hint ? <span className={cn(field.obvious_hint && 'font-semibold text-warn')}>{field.hint}</span> : undefined
  const [listEditor] = useState(
    () => field.secret || !!field.options?.length || !Array.isArray(value) || value.every((entry) => typeof entry === 'string'),
  )

  if (field.type === 'bool') {
    return (
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <label htmlFor={id} className="text-sm font-semibold text-ink">
            {label}
          </label>
          {hint && (
            <p id={`${id}-hint`} className="mt-1 text-[13px] leading-relaxed text-ink-3">
              {hint}
            </p>
          )}
        </div>
        <Switch id={id} checked={value === true} onCheckedChange={onChange} />
      </div>
    )
  }

  if (field.type === 'object') {
    return (
      <fieldset className="min-w-0 border-t border-paper-3">
        <legend className="pr-3 text-sm font-semibold text-ink">{label}</legend>
        {hint && <p className="mt-1 text-[13px] leading-relaxed text-ink-3">{hint}</p>}
        <div className="pt-3">
          <SchemaForm schema={field.items ?? {}} value={isObject(value) ? value : {}} onChange={onChange} onValidity={onValidity} />
        </div>
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
        format={(next) => next.toFixed(decimals)}
        onChange={onChange}
      />
    )
  }

  const values = Array.isArray(value) ? value : []
  if (field.type === 'list' && listEditor) {
    return <ListField name={name} label={label} hint={hint} values={values} field={field} onChange={onChange} />
  }
  if (field.type === 'template_list' && isObject(field.templates) && Object.keys(field.templates).length) {
    return (
      <TemplateList
        id={id}
        label={label}
        hint={hint}
        templates={field.templates}
        values={values}
        onChange={onChange}
        onValidity={onValidity}
      />
    )
  }
  if (field.type === 'file') {
    return (
      <Field label={label} hint="此项需要文件，控制台暂不支持编辑。">
        {null}
      </Field>
    )
  }

  const options = field.options?.length && !field.secret ? field.options : undefined
  if (options && (field.type === 'string' || field.type === 'text' || field.type === 'int' || field.type === 'float')) {
    const selected = options.indexOf(value as string | number)
    return (
      <Field label={label} htmlFor={id} hint={hint}>
        <Select id={id} value={selected < 0 ? '' : selected} onChange={(event) => onChange(options[Number(event.target.value)])}>
          <option value="" disabled>
            请选择
          </option>
          {options.map((option, index) => (
            <option key={index} value={index}>
              {field.labels?.[index] ?? option}
            </option>
          ))}
        </Select>
      </Field>
    )
  }

  if (field.type === 'int' || field.type === 'float' || field.type === 'dict' || field.type === 'list' || field.type === 'template_list') {
    return <DraftField id={id} kind={field.type} value={value} label={label} hint={hint} onChange={onChange} onValidity={onValidity} />
  }

  return (
    <Field
      label={
        field.secret ? (
          <span className="inline-flex items-center gap-1.5">
            {label}
            <LockKeyhole className="size-3.5" aria-hidden />
          </span>
        ) : (
          label
        )
      }
      htmlFor={id}
      hint={hint}
    >
      {!field.secret && (field.type === 'text' || field.editor_mode) ? (
        <Textarea
          id={id}
          value={String(value ?? '')}
          onChange={(event) => onChange(event.target.value)}
          className={cn(field.editor_mode && 'font-mono text-[13px]')}
        />
      ) : (
        <Input
          id={id}
          type={field.secret ? 'password' : 'text'}
          autoComplete={field.secret ? 'off' : undefined}
          spellCheck={!field.secret}
          value={String(value ?? '')}
          onChange={(event) => onChange(event.target.value)}
        />
      )}
    </Field>
  )
}

export function SchemaForm({
  schema,
  value,
  onChange,
  onValidity,
}: {
  schema: PluginSchema
  value: Obj
  onChange: (value: Obj) => void
  onValidity: SetValidity
}) {
  const fields = schemaFields(schema).filter(
    ([, field]) =>
      !field.invisible && (!field.condition || Object.entries(field.condition).every(([key, expected]) => value[key] === expected)),
  )
  if (!fields.length) return <p className="text-sm text-ink-3">没有可编辑的设置。</p>

  return (
    <div className="grid gap-5">
      {fields.map(([name, field]) => (
        <FieldInput
          key={name}
          name={name}
          field={field}
          value={value[name]}
          onChange={(next) => onChange({ ...value, [name]: next })}
          onValidity={onValidity}
        />
      ))}
    </div>
  )
}
