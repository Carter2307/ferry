import { renderToStaticMarkup } from 'react-dom/server'
import { MemoryRouter } from 'react-router'
import { describe, expect, it } from 'vitest'

import { TooltipProvider } from '@/components/ui/tooltip'
import type { InstanceStatus } from '@/lib/api/types'

import { InstanceCard } from './InstancesSection'

const MiB = 1024 * 1024

function instance(patch: Partial<InstanceStatus>): InstanceStatus {
  return {
    container_id: 'c0ffee',
    name: 'ferry-web-dep1-a1b2c3',
    deploy_id: 'dep-1',
    state: 'running',
    host_port: 49152,
    started_at: null,
    restart_count: 0,
    cpu_percent: 12.5,
    memory_bytes: 128 * MiB,
    memory_limit_bytes: 512 * MiB,
    cpu_limit: 0.5,
    oom_killed: false,
    exit_code: null,
    ...patch,
  }
}

/** Visible text of an instance card (tags stripped, whitespace collapsed). */
function render(inst: InstanceStatus): { html: string; text: string } {
  const html = renderToStaticMarkup(
    <MemoryRouter>
      <TooltipProvider>
        <InstanceCard instance={inst} samples={[]} settingsPath="/services/web/settings" />
      </TooltipProvider>
    </MemoryRouter>,
  )
  const text = html
    .replace(/<[^>]+>/g, ' ')
    .replace(/&#x27;/g, "'")
    .replace(/\s+/g, ' ')
    .trim()
  return { html, text }
}

describe('InstanceCard', () => {
  it('shows memory use against the configured limit, and the CPU limit', () => {
    const { text, html } = render(instance({}))
    expect(text).toContain('128 MiB')
    expect(text).toContain('limit 512 MiB')
    expect(text).toContain('25.0% of 512 MiB')
    expect(text).toContain('limit 0.5 CPU')
    expect(html).toContain('aria-valuenow="25"')
    expect(text).not.toMatch(/OOM killed/i)
  })

  it('says "no limit" for unlimited containers', () => {
    const { text, html } = render(instance({ memory_limit_bytes: null, cpu_limit: null }))
    expect(text).toContain('no limit')
    expect(text).not.toContain('limit 512 MiB')
    expect(text).not.toContain('% of')
    expect(html).toContain('aria-valuenow="0"')
  })

  it('flags OOM-killed instances with the exit code and a link to the limits', () => {
    const { text, html } = render(
      instance({ state: 'restarting', oom_killed: true, exit_code: 137, memory_bytes: null, restart_count: 3 }),
    )
    expect(text).toMatch(/OOM killed · exit 137/)
    expect(text).toContain('Ran out of memory (limit 512 MiB).')
    expect(html).toContain('href="/services/web/settings#resources"')
    // The state pill doesn't repeat the exit code already in the OOM badge.
    expect(text).not.toMatch(/restarting · 137/)
    expect(html).toContain('text-destructive')
  })

  it('shows the exit code of a stopped instance that was not OOM-killed', () => {
    const { text } = render(instance({ state: 'exited', exit_code: 1, memory_bytes: null }))
    expect(text).toContain('exited · 1')
    expect(text).not.toMatch(/OOM killed/i)
  })
})
