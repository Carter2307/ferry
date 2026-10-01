import * as React from 'react'
import { AlertTriangle, Eye, EyeOff } from 'lucide-react'

import { FerryLogo } from '@/components/patterns/icons'
import { ThemeMenu } from '@/components/shell/TopBar'
import { Input, type InputProps } from '@/components/ui/input'

/**
 * The pages shown before the dashboard: create the account, sign in. A form
 * column on the left, what Ferry does on the right (wide screens).
 */
export function AuthLayout({
  title,
  description,
  children,
}: {
  title: string
  description: React.ReactNode
  children: React.ReactNode
}) {
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
              <h1 className="text-[28px] leading-tight font-medium tracking-[-0.01em] text-foreground">{title}</h1>
              <p className="text-sm text-foreground-light">{description}</p>
            </div>
            {children}
          </div>
        </main>
      </div>
      <aside
        className="relative hidden flex-1 items-center justify-center overflow-hidden bg-surface-75 lg:flex"
        aria-hidden="true"
      >
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

/** A labelled field of an auth form, with its hint or error underneath. */
export function Field({
  id,
  label,
  hint,
  error,
  children,
}: {
  id: string
  label: string
  hint?: React.ReactNode
  error?: string | null
  children: React.ReactNode
}) {
  return (
    <div className="flex flex-col gap-2">
      <label htmlFor={id} className="text-sm text-foreground-light">
        {label}
      </label>
      {children}
      {error ? (
        <p id={`${id}-error`} role="alert" className="text-[13px] text-destructive">
          {error}
        </p>
      ) : (
        hint && (
          <p id={`${id}-hint`} className="text-[13px] leading-relaxed text-foreground-lighter">
            {hint}
          </p>
        )
      )}
    </div>
  )
}

/** Password input with a show / hide button. */
export function PasswordInput(props: Omit<InputProps, 'type'>) {
  const [show, setShow] = React.useState(false)
  return (
    <div className="relative">
      <Input {...props} type={show ? 'text' : 'password'} spellCheck={false} className="pr-10" />
      <button
        type="button"
        onClick={() => setShow((s) => !s)}
        aria-label={show ? 'Hide password' : 'Show password'}
        aria-pressed={show}
        className="absolute top-1/2 right-2 flex size-7 -translate-y-1/2 cursor-pointer items-center justify-center rounded-md text-foreground-lighter outline-none hover:bg-surface-200 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
      >
        {show ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
      </button>
    </div>
  )
}

/** What the server answered, when the form can't say it next to a field. */
export function FormError({ children }: { children: React.ReactNode }) {
  return (
    <div
      role="alert"
      className="flex items-start gap-2 rounded-md border border-destructive-border bg-destructive-soft p-3 text-[13px] text-foreground"
    >
      <AlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
      <span className="break-words">{children}</span>
    </div>
  )
}
