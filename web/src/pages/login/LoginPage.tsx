import * as React from 'react'
import { Navigate, useNavigate, useSearchParams } from 'react-router'
import { ArrowRight } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ApiError, errorMessage } from '@/lib/api/client'
import { endpoints } from '@/lib/api/endpoints'
import { useAuth } from '@/stores/auth'

import { AuthLayout, Field, FormError, PasswordInput } from './AuthLayout'
import { fieldAria } from './forms'
import { safeNext } from './safeNext'

/** `/login` — sign in to the server's account with its email and password. */
export function LoginPage() {
  const phase = useAuth((s) => s.phase)
  const apply = useAuth((s) => s.apply)
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const next = safeNext(params.get('next'))

  const [email, setEmail] = React.useState('')
  const [password, setPassword] = React.useState('')
  const [pending, setPending] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const emailId = React.useId()
  const passwordId = React.useId()

  // A server without an account: create it first.
  if (phase === 'setup') return <Navigate to="/setup" replace />
  if (phase === 'signed-in' && !pending) return <Navigate to={next} replace />

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (pending) return
    if (!email.trim() || !password) {
      setError('Enter your email and your password.')
      return
    }
    setPending(true)
    setError(null)
    try {
      apply(await endpoints.login({ email: email.trim(), password }))
      void navigate(next, { replace: true })
    } catch (err) {
      setError(
        err instanceof ApiError && err.code === 'invalid_credentials' ? 'Wrong email or password.' : errorMessage(err),
      )
      setPending(false)
    }
  }

  return (
    <AuthLayout title="Welcome back" description="Sign in to your Ferry server.">
      <form onSubmit={(e) => void submit(e)} noValidate className="flex flex-col gap-5">
        <Field id={emailId} label="Email">
          <Input
            id={emailId}
            type="email"
            size="lg"
            autoFocus
            autoComplete="username"
            spellCheck={false}
            placeholder="you@example.com"
            value={email}
            onChange={(e) => {
              setEmail(e.target.value)
              if (error) setError(null)
            }}
            {...fieldAria(emailId, null, null)}
          />
        </Field>
        <Field id={passwordId} label="Password">
          <PasswordInput
            id={passwordId}
            size="lg"
            autoComplete="current-password"
            value={password}
            onChange={(e) => {
              setPassword(e.target.value)
              if (error) setError(null)
            }}
          />
        </Field>
        {error && <FormError>{error}</FormError>}
        <Button
          type="submit"
          variant="primary"
          size="lg"
          loading={pending}
          className="w-full"
          iconRight={!pending ? <ArrowRight /> : undefined}
        >
          Sign in
        </Button>
      </form>
      <p className="text-[13px] leading-relaxed text-foreground-lighter">
        Forgot your password? Run <code className="font-mono text-foreground-light">ferryd reset-password</code> on the
        server to choose a new one.
      </p>
    </AuthLayout>
  )
}
