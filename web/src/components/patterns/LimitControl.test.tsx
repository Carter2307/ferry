import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'

import type { ServerInfo } from '@/lib/api/types'
import type { LimitField } from '@/lib/resources'

import { LimitControl } from './LimitControl'

const info: ServerInfo = {
  version: '0.1.0',
  base_domain: 'localhost',
  proxy_url: 'http://localhost:8080',
  tls_enabled: false,
  dashboard_url: null,
  github_webhook_enabled: false,
  docker_version: '28.0.0',
  default_memory_limit_mb: 512,
  default_cpu_limit: 1,
  docker_cpus: 4,
  docker_memory_bytes: null,
}

function render(kind: 'memory' | 'cpu', value: LimitField): string {
  return renderToStaticMarkup(
    <LimitControl kind={kind} id="limit" value={value} onChange={() => {}} info={info} invalid={false} />,
  )
}

describe('LimitControl', () => {
  it('renders only the select for the server default and presets', () => {
    expect(render('memory', { choice: 'default', custom: '' })).not.toContain('limit-custom')
    expect(render('cpu', { choice: '0.5', custom: '' })).not.toContain('limit-custom')
  })

  it('shows the custom input with the parsed value', () => {
    const html = render('memory', { choice: 'custom', custom: '1.5G' })
    expect(html).toContain('id="limit-custom"')
    expect(html).toContain('value="1.5G"')
    expect(html).toContain('aria-label="Custom memory limit"')
    expect(html).toContain('= 1.5 GiB')

    const cpu = render('cpu', { choice: 'custom', custom: '1500m' })
    expect(cpu).toContain('= 1.5 CPUs')
  })

  it('shows no parsed value for an invalid custom size', () => {
    const html = render('memory', { choice: 'custom', custom: 'lots' })
    expect(html).toContain('value="lots"')
    expect(html).not.toContain('= ')
  })
})
