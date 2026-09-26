import * as React from 'react'
import { Navigate, useNavigate, useSearchParams } from 'react-router'
import { ArrowRight, Eye, EyeOff, KeyRound } from 'lucide-react'

import { FerryLogo } from '@/components/patterns/icons'
import { MonoLabel } from '@/components/patterns/MonoLabel'
import { ThemeMenu } from '@/components/shell/TopBar'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ApiError, errorMessage, request } from '@/lib/api/client'
import type { ServerInfo } from '@/lib/api/types'
import { useAuth } from '@/stores/auth'

import { safeNext } from './safeNext'

/** `/login` — paste the API token printed by ferryd; validated with GET /api/v1/info. */
export function LoginPage() {
  const token = useAuth((s) => s.token)
  const signIn = useAuth((s) => s.signIn)
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const next = safeNext(params.get('next'))

  const [value, setValue] = React.useState('')
  const [show, setShow] = React.useState(false)
  const [pending, setPending] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const inputId = React.useId()
  const errorId = React.useId()

  if (token && !pending) return <Navigate to={next} replace />

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    const candidate = value.trim()
    if (!candidate) {
      setError('Enter your API token.')
      return
    }
    setPending(true)
    setError(null)
    try {
      await request<ServerInfo>('/api/v1/info', { token: candidate, skipAuthRedirect: true })
      signIn(candidate)
      void navigate(next, { replace: true })
    } catch (err) {
      setError(
        err instanceof ApiError && err.status === 401
          ? 'This token was rejected by the server.'
          : errorMessage(err),
      )
      setPending(false)
    }
  }

  return (
    <div className="flex min-h-dvh bg-background">
      <div className="flex w-full flex-col lg:w-[520px] lg:shrink-0 lg:border-r xl:w-[560px]">
        <header className="flex h-16 items-center justify-between px-6 sm:px-10">
          <span className="flex items-center gap-2 text-[15px] font-medium text-foreground">
            <FerryLogo className="size-6" />
            Ferry
          </span>
          <ThemeMenu />
        </header>
        <main className="flex flex-1 items-center px-6 pb-16 sm:px-10">
          <div className="mx-auto flex w-full max-w-[384px] flex-col gap-8">
            <div className="flex flex-col gap-2">
              <h1 className="text-[28px] leading-tight font-medium tracking-[-0.01em] text-foreground">Welcome back</h1>
              <p className="text-sm text-foreground-light">Sign in to your Ferry server with its API token.</p>
            </div>
            <form onSubmit={(e) => void submit(e)} noValidate className="flex flex-col gap-5">
              <div className="flex flex-col gap-2">
                <label htmlFor={inputId} className="text-sm text-foreground-light">
                  API token
                </label>
                <div className="relative">
                  <KeyRound
                    className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-foreground-lighter"
                    aria-hidden="true"
                  />
                  <Input
                    id={inputId}
                    type={show ? 'text' : 'password'}
                    size="lg"
                    mono
                    autoFocus
                    autoComplete="current-password"
                    spellCheck={false}
                    placeholder="Paste your API token"
                    value={value}
                    onChange={(e) => {
                      setValue(e.target.value)
                      if (error) setError(null)
                    }}
                    aria-invalid={error ? true : undefined}
                    aria-describedby={error ? errorId : undefined}
                    className="pr-10 pl-9"
                  />
                  <button
                    type="button"
                    onClick={() => setShow((s) => !s)}
                    aria-label={show ? 'Hide token' : 'Show token'}
                    aria-pressed={show}
                    className="absolute top-1/2 right-2 flex size-7 -translate-y-1/2 cursor-pointer items-center justify-center rounded-md text-foreground-lighter outline-none hover:bg-surface-200 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    {show ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                  </button>
                </div>
                {error && (
                  <p id={errorId} role="alert" className="text-[13px] text-destructive">
                    {error}
                  </p>
                )}
              </div>
              <Button type="submit" variant="primary" size="lg" loading={pending} className="w-full" iconRight={!pending ? <ArrowRight /> : undefined}>
                Sign in
              </Button>
            </form>
            <div className="flex flex-col gap-2 rounded-lg border border-dashed border-border-stronger p-4">
              <MonoLabel>Where is my token?</MonoLabel>
              <p className="text-[13px] leading-relaxed text-foreground-light">
                <code className="font-mono text-foreground">ferryd</code> prints it on first start and stores it in{' '}
                <code className="font-mono text-foreground">&lt;data-dir&gt;/api_token</code> (or set{' '}
                <code className="font-mono text-foreground">FERRY_API_TOKEN</code>).
              </p>
            </div>
          </div>
        </main>
      </div>
      <aside className="relative hidden flex-1 items-center justify-center overflow-hidden bg-surface-75 lg:flex" aria-hidden="true">
        <div className="bg-dot-grid absolute inset-0 opacity-70" />
        <div className="relative flex max-w-md flex-col gap-6 px-10">
          <p className="text-[26px] leading-snug font-medium tracking-[-0.01em] text-foreground">
            Push to deploy on your own machine. Web services, workers, cron jobs, static sites, Postgres and Redis.
          </p>
          <div className="rounded-lg border bg-surface-100 p-4 shadow-card">
            <div className="mb-2 flex items-center gap-2">
              <span className="size-2 rounded-full bg-destructive-solid/70" />
              <span className="size-2 rounded-full bg-warning/70" />
              <span className="size-2 rounded-full bg-brand/80" />
            </div>
            <pre className="font-mono text-[12.5px] leading-relaxed text-foreground-light">
              <span className="text-foreground-muted">$ </span>ferry up my-app --follow{'\n'}
              <span className="text-primary">==&gt; Building with node runtime</span>
              {'\n'}
              <span className="text-success">==&gt; Deploy live</span>
              {'\n'}
              <span className="text-foreground">http://my-app.localhost</span>
            </pre>
          </div>
        </div>
      </aside>
    </div>
  )
}
