/**
 * Container resource limits — mirror of `ferry-core/src/resources.rs`: the
 * accepted ranges, parsing / formatting of human sizes (`512M`, `1.5G`,
 * `0.5`, `500m`), and the preset + custom form model behind the Memory limit
 * / CPU limit controls.
 *
 * Units: memory in MiB, CPU in CPUs (`0.5` = half a core). Like Docker,
 * `M`/`MB`/`MiB` all mean MiB and `G`/`GB`/`GiB` all mean GiB.
 */

import type { ServerInfo } from '@/lib/api/types'
import { bytes } from '@/lib/format'

/** Smallest accepted memory limit (MiB). */
export const MIN_MEMORY_LIMIT_MB = 16
/** Largest accepted memory limit (MiB): 1 TiB. */
export const MAX_MEMORY_LIMIT_MB = 1024 * 1024
/** Smallest accepted CPU limit (CPUs). */
export const MIN_CPU_LIMIT = 0.01
/** Largest accepted CPU limit (CPUs). */
export const MAX_CPU_LIMIT = 512

/** Memory presets offered by the select (MiB). */
export const MEMORY_PRESETS_MB: readonly number[] = [256, 512, 1024, 2048, 4096, 8192]
/** CPU presets offered by the select (CPUs). */
export const CPU_PRESETS: readonly number[] = [0.25, 0.5, 1, 2, 4]

const U32_MAX = 4294967295
const DECIMAL = /^(\d+\.?\d*|\.\d+)$/

/** `trim_float`: at most 2 decimals, trailing zeros dropped (`1.50` → `1.5`). */
function trimFloat(v: number): string {
  return v.toFixed(2).replace(/0+$/, '').replace(/\.$/, '')
}

/** `round_cpus`: Docker's CLI granularity (0.01 CPU). */
export function roundCpus(cpus: number): number {
  return Math.round(cpus * 100) / 100
}

/**
 * `parse_memory_mb`: `512`, `512M`, `512MiB`, `1G`, `1.5GB`, `2g` → MiB. A
 * plain number is MiB; `0` parses to 0. Returns null when unparseable.
 */
export function parseMemoryMb(input: string): number | null {
  const t = input.trim().toLowerCase()
  const split = t.search(/[^\d.]/)
  const num = split < 0 ? t : t.slice(0, split)
  const unit = split < 0 ? '' : t.slice(split).trim()
  if (!DECIMAL.test(num)) return null
  const value = Number(num)
  const factors: Record<string, number> = {
    '': 1,
    m: 1,
    mb: 1,
    mib: 1,
    g: 1024,
    gb: 1024,
    gib: 1024,
    t: 1024 * 1024,
    tb: 1024 * 1024,
    tib: 1024 * 1024,
    k: 1 / 1024,
    kb: 1 / 1024,
    kib: 1 / 1024,
  }
  const factor = factors[unit]
  if (factor === undefined) return null
  const mb = Math.round(value * factor)
  if (!Number.isFinite(mb) || mb < 0 || mb > U32_MAX || (value > 0 && mb === 0)) return null
  return mb
}

/**
 * `format_memory_mb`: `512 MiB`, `1 GiB`, `1.5 GiB`, `1152 MiB`. GiB only for
 * quarter-GiB multiples, exact at 2 decimals (so no rounding differs from Rust's).
 */
export function formatMemoryMb(mb: number): string {
  if (mb >= 1024 && mb % 256 === 0) return `${trimFloat(mb / 1024)} GiB`
  return `${mb} MiB`
}

/** `parse_cpus`: `0.5`, `2`, `1 cpu`, or millicores `500m`, rounded to 0.01. Null when unparseable. */
export function parseCpus(input: string): number | null {
  const t = input.trim().toLowerCase()
  let value: number
  if (t.endsWith('m')) {
    const millis = t.slice(0, -1).trim()
    if (!DECIMAL.test(millis)) return null
    value = Number(millis) / 1000
  } else {
    const n = t.replace(/(cpus)+$/, '').replace(/(cpu)+$/, '').trim()
    if (!DECIMAL.test(n)) return null
    value = Number(n)
  }
  if (!Number.isFinite(value) || value < 0) return null
  return roundCpus(value)
}

/** `format_cpus`: `0.5 CPU`, `1 CPU`, `2 CPUs`. */
export function formatCpus(cpus: number): string {
  const n = trimFloat(cpus)
  return cpus > 1 ? `${n} CPUs` : `${n} CPU`
}

/** `validate_memory_mb` → message, or null when accepted. */
export function memoryLimitError(mb: number): string | null {
  if (Number.isInteger(mb) && mb >= MIN_MEMORY_LIMIT_MB && mb <= MAX_MEMORY_LIMIT_MB) return null
  return `Use between ${MIN_MEMORY_LIMIT_MB} MiB and ${formatMemoryMb(MAX_MEMORY_LIMIT_MB)}.`
}

/**
 * `validate_cpus` → message, or null when accepted. With the Docker host's
 * CPU count, also refuses more CPUs than the host has (Docker would reject
 * the container).
 */
export function cpuLimitError(cpus: number, hostCpus?: number | null): string | null {
  if (!Number.isFinite(cpus) || cpus < MIN_CPU_LIMIT || cpus > MAX_CPU_LIMIT) {
    return `Use between ${MIN_CPU_LIMIT} and ${MAX_CPU_LIMIT} CPUs.`
  }
  if (hostCpus && cpus > hostCpus) return `The Docker host has ${formatCpus(hostCpus)}; use at most ${hostCpus}.`
  return null
}

// ---------------------------------------------------------------------------
// Server defaults and effective limits
// ---------------------------------------------------------------------------

type Defaults = Pick<ServerInfo, 'default_memory_limit_mb' | 'default_cpu_limit'> | null | undefined

/** `512 MiB`, `Unlimited` (default 0), or null while the server info is unknown. */
export function defaultMemoryText(info: Defaults): string | null {
  if (!info) return null
  return info.default_memory_limit_mb > 0 ? formatMemoryMb(info.default_memory_limit_mb) : 'Unlimited'
}

/** `1 CPU`, `Unlimited` (default 0), or null while the server info is unknown. */
export function defaultCpuText(info: Defaults): string | null {
  if (!info) return null
  return info.default_cpu_limit > 0 ? formatCpus(info.default_cpu_limit) : 'Unlimited'
}

/** Effective memory limit of a resource: its own value, else the server default (`Server default (512 MiB)`). */
export function effectiveMemoryText(mb: number | null | undefined, info: Defaults): string {
  if (mb) return formatMemoryMb(mb)
  const d = defaultMemoryText(info)
  return d ? `Server default (${d})` : 'Server default'
}

/** Effective CPU limit of a resource: its own value, else the server default (`Server default (1 CPU)`). */
export function effectiveCpuText(cpus: number | null | undefined, info: Defaults): string {
  if (cpus) return formatCpus(cpus)
  const d = defaultCpuText(info)
  return d ? `Server default (${d})` : 'Server default'
}

// ---------------------------------------------------------------------------
// Form model: a select (server default · presets · Custom…) + a custom input
// ---------------------------------------------------------------------------

export const LIMIT_DEFAULT = 'default'
export const LIMIT_CUSTOM = 'custom'

/**
 * State of one limit control. `choice` is `default`, `custom`, or a preset
 * value as a string (`'512'`, `'0.5'`); `custom` is the text of the custom
 * input (kept while another choice is selected).
 */
export interface LimitField {
  choice: string
  custom: string
}

export const DEFAULT_LIMIT: LimitField = { choice: LIMIT_DEFAULT, custom: '' }

export type LimitKind = 'memory' | 'cpu'

/** Custom-input text for a stored MiB value (round-trips through {@link parseMemoryMb}). */
function memoryText(mb: number): string {
  const pretty = formatMemoryMb(mb)
  return parseMemoryMb(pretty) === mb ? pretty : `${mb} MiB`
}

/** Form state for a stored value (`null`/0 = server default). */
export function limitFieldFrom(kind: LimitKind, value: number | null | undefined): LimitField {
  if (!value) return DEFAULT_LIMIT
  const presets = kind === 'memory' ? MEMORY_PRESETS_MB : CPU_PRESETS
  if (presets.includes(value)) return { choice: String(value), custom: '' }
  return { choice: LIMIT_CUSTOM, custom: kind === 'memory' ? memoryText(value) : String(value) }
}

/** Resolved value of a limit control: `null` = server default. */
export type LimitValue = { ok: true; value: number | null } | { ok: false; error: string }

/**
 * Parse and validate a limit control. `hostCpus` (Docker host CPUs, when
 * known) caps CPU limits.
 */
export function readLimit(kind: LimitKind, field: LimitField, hostCpus?: number | null): LimitValue {
  if (field.choice === LIMIT_DEFAULT) return { ok: true, value: null }
  const raw = field.choice === LIMIT_CUSTOM ? field.custom.trim() : field.choice
  if (kind === 'memory') {
    if (!raw) return { ok: false, error: 'Enter a size, e.g. 768M or 1.5G.' }
    const mb = parseMemoryMb(raw)
    if (mb === null) return { ok: false, error: `“${raw}” is not a size. Examples: 768M, 1.5G, 3072.` }
    const error = memoryLimitError(mb)
    return error ? { ok: false, error } : { ok: true, value: mb }
  }
  if (!raw) return { ok: false, error: 'Enter a number of CPUs, e.g. 0.75 or 1500m.' }
  const cpus = parseCpus(raw)
  if (cpus === null) return { ok: false, error: `“${raw}” is not a CPU amount. Examples: 0.75, 3, 1500m.` }
  const error = cpuLimitError(cpus, hostCpus)
  return error ? { ok: false, error } : { ok: true, value: cpus }
}

/** Validation message of a limit control, or null. */
export function limitError(kind: LimitKind, field: LimitField, hostCpus?: number | null): string | null {
  const r = readLimit(kind, field, hostCpus)
  return r.ok ? null : r.error
}

/** Stored value of a valid limit control (`null` = server default; invalid → null too). */
export function limitValue(kind: LimitKind, field: LimitField): number | null {
  const r = readLimit(kind, field)
  return r.ok ? r.value : null
}

/** One option of the limit select. */
export interface LimitOption {
  value: string
  label: string
  /** Shown dimmed after the label (e.g. "more than the host"). */
  note?: string
  disabled?: boolean
}

/**
 * Select options: `Server default (512 MiB)`, the presets, `Custom…`. CPU
 * presets above the Docker host's CPUs are disabled; memory presets above
 * its memory are flagged (allowed, but the host can't back them).
 */
export function limitOptions(kind: LimitKind, info: ServerInfo | null | undefined): LimitOption[] {
  const d = kind === 'memory' ? defaultMemoryText(info) : defaultCpuText(info)
  const options: LimitOption[] = [{ value: LIMIT_DEFAULT, label: d ? `Server default (${d})` : 'Server default' }]
  if (kind === 'memory') {
    const hostMb = info?.docker_memory_bytes ? info.docker_memory_bytes / (1024 * 1024) : null
    for (const mb of MEMORY_PRESETS_MB) {
      options.push({
        value: String(mb),
        label: formatMemoryMb(mb),
        note: hostMb !== null && mb > hostMb ? 'more than the host' : undefined,
      })
    }
  } else {
    const host = info?.docker_cpus ?? null
    for (const c of CPU_PRESETS) {
      const over = host !== null && c > host
      options.push({ value: String(c), label: formatCpus(c), note: over ? 'more than the host' : undefined, disabled: over })
    }
  }
  options.push({ value: LIMIT_CUSTOM, label: 'Custom…' })
  return options
}

/**
 * New state after picking `choice` in the select. Switching to Custom
 * pre-fills the input with the previous preset so it can be tweaked.
 */
export function chooseLimit(kind: LimitKind, field: LimitField, choice: string): LimitField {
  if (choice === LIMIT_CUSTOM && field.custom.trim() === '' && field.choice !== LIMIT_DEFAULT && field.choice !== LIMIT_CUSTOM) {
    const preset = Number(field.choice)
    return { choice, custom: kind === 'memory' ? memoryText(preset) : String(preset) }
  }
  return { ...field, choice }
}

/**
 * Whether a settings form shows the error of a limit control: after a submit,
 * or once the user changed it — except a Custom… input still empty (just
 * picked, nothing typed yet).
 */
export function showLimitError(field: LimitField, base: LimitField, submitted: boolean): boolean {
  if (submitted) return true
  if (field.choice === LIMIT_CUSTOM && field.custom.trim() === '') return false
  return field.choice !== base.choice || field.custom !== base.custom
}

/** Warning (not an error) when a memory limit exceeds the Docker host's memory. */
export function memoryHostWarning(mb: number | null, info: ServerInfo | null | undefined): string | null {
  if (!mb || !info?.docker_memory_bytes) return null
  return mb * 1024 * 1024 > info.docker_memory_bytes
    ? `More than the Docker host’s memory (${bytes(info.docker_memory_bytes)}): the host runs out before the limit is reached.`
    : null
}

// ---------------------------------------------------------------------------
// Settings forms (service Resources section, datastore Resources section)
// ---------------------------------------------------------------------------

/** Flat form state of both limits (shallow-comparable, for `useSyncedForm`). */
export interface LimitsForm {
  memory: string
  memoryCustom: string
  cpu: string
  cpuCustom: string
}

export const memoryFieldOf = (f: LimitsForm): LimitField => ({ choice: f.memory, custom: f.memoryCustom })
export const cpuFieldOf = (f: LimitsForm): LimitField => ({ choice: f.cpu, custom: f.cpuCustom })

export function limitsFormFrom(r: { memory_limit_mb?: number | null; cpu_limit?: number | null }): LimitsForm {
  const m = limitFieldFrom('memory', r.memory_limit_mb)
  const c = limitFieldFrom('cpu', r.cpu_limit)
  return { memory: m.choice, memoryCustom: m.custom, cpu: c.choice, cpuCustom: c.custom }
}

/**
 * Errors of a limits form. The Docker host's CPU count only caps a CPU limit
 * the user changed: a stored value above it (set from the CLI or a
 * blueprint) doesn't block saving the memory limit.
 */
export function validateLimitsForm(
  f: LimitsForm,
  base: LimitsForm,
  hostCpus?: number | null,
): Partial<Record<'memory' | 'cpu', string>> {
  const errors: Partial<Record<'memory' | 'cpu', string>> = {}
  const memory = limitError('memory', memoryFieldOf(f))
  if (memory) errors.memory = memory
  const cpu = readLimit('cpu', cpuFieldOf(f))
  if (!cpu.ok) errors.cpu = cpu.error
  else if (cpu.value !== limitValue('cpu', cpuFieldOf(base))) {
    const capped = cpu.value === null ? null : cpuLimitError(cpu.value, hostCpus)
    if (capped) errors.cpu = capped
  }
  return errors
}

/** Minimal PATCH body (`UpdateService` / `UpdateDatastore` fields): changed limits only, 0 = back to the default. */
export function limitsPatch(base: LimitsForm, next: LimitsForm): { memory_limit_mb?: number; cpu_limit?: number } {
  const patch: { memory_limit_mb?: number; cpu_limit?: number } = {}
  const memory = limitValue('memory', memoryFieldOf(next))
  if (memory !== limitValue('memory', memoryFieldOf(base))) patch.memory_limit_mb = memory ?? 0
  const cpu = limitValue('cpu', cpuFieldOf(next))
  if (cpu !== limitValue('cpu', cpuFieldOf(base))) patch.cpu_limit = cpu ?? 0
  return patch
}

/** One-line summary of a patch for toasts: `Memory 1 GiB · CPU server default`. */
export function describeLimitsPatch(patch: { memory_limit_mb?: number; cpu_limit?: number }): string {
  const parts: string[] = []
  if (patch.memory_limit_mb !== undefined) {
    parts.push(`memory ${patch.memory_limit_mb ? formatMemoryMb(patch.memory_limit_mb) : 'server default'}`)
  }
  if (patch.cpu_limit !== undefined) parts.push(`CPU ${patch.cpu_limit ? formatCpus(patch.cpu_limit) : 'server default'}`)
  const s = parts.join(' · ')
  return s.charAt(0).toUpperCase() + s.slice(1)
}
