import * as React from 'react'
import { Navigate, useNavigate, useSearchParams } from 'react-router'
import { ArrowRight } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ApiError, errorMessage } from '@/lib/api/client'
import { endpoints } from '@/lib/api/endpoints'
import { useAuth } from '@/stores/auth'

import { AuthLayout, Field, FormError, PasswordInput } from './AuthLayout'
import { fieldAria, MIN_PASSWORD_LENGTH, passwordError } from './forms'

const CODE_HINT = (
  <>
    The code of the link <code className="font-mono text-foreground-light">ferryd</code> printed when it started. Run{' '}
    <code className="font-mono text-foreground-light">ferryd status</code> on the server to see the link again.
  </>
)

/**
 * `/setup?code=…` — the first run of a server: create the account that
 * administers it. The setup code proves that whoever fills this in can read
 * the server's terminal; the link ferryd prints carries it.
 */
export function SetupPage() {
  const phase = useAuth((s) => s.phase)
  const apply = useAuth((s) => s.apply)
  const [params] = useSearchParams()
  const navigate = useNavigate()

  const [email, setEmail] = React.useState('')
  const [password, setPassword] = React.useState('')
  const [code, setCode] = React.useState(() => params.get('code')?.trim() ?? '')
  const [touched, setTouched] = React.useState(false)
  const [pending, setPending] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const [codeError, setCodeError] = React.useState<string | null>(null)
  const emailId = React.useId()
  const passwordId = React.useId()
  const codeId = React.useId()

  // The account exists: this page has nothing left to do.
  if (phase !== 'setup' && !pending) return <Navigate to={phase === 'signed-in' ? '/services' : '/login'} replace />

  const emailError = email.trim() === '' ? 'Enter your email.' : null
  const newPasswordError = passwordError(password)
  const missingCode = code.trim() === '' ? 'Enter the setup code.' : null

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setTouched(true)
    if (pending || emailError || newPasswordError || missingCode) return
    setPending(true)
    setError(null)
    setCodeError(null)
    try {
      const status = await endpoints.setupAccount({ email: email.trim(), password, code: code.trim() })
      apply(status)
      // `replace`: the setup code doesn't stay in the history.
      void navigate('/services', { replace: true })
    } catch (err) {
      if (err instanceof ApiError && err.code === 'invalid_setup_code') {
        setCodeError('This is not the setup code of this server.')
      } else {
        setError(errorMessage(err))
      }
      setPending(false)
    }
  }

  const shownCodeError = codeError ?? (touched ? missingCode : null)
  return (
    <AuthLayout
      title="Create your account"
      description="This server has no account yet. The one you create here administers it."
    >
      <form onSubmit={(e) => void submit(e)} noValidate className="flex flex-col gap-5">
        <Field id={emailId} label="Email" error={touched ? emailError : null}>
          <Input
            id={emailId}
            type="email"
            size="lg"
            autoFocus
            autoComplete="username"
            spellCheck={false}
            placeholder="you@example.com"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            {...fieldAria(emailId, null, touched ? emailError : null)}
          />
        </Field>
        <Field
          id={passwordId}
          label="Password"
          hint={`At least ${MIN_PASSWORD_LENGTH} characters.`}
          error={touched ? newPasswordError : null}
        >
          <PasswordInput
            id={passwordId}
            size="lg"
            autoComplete="new-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            {...fieldAria(passwordId, true, touched ? newPasswordError : null)}
          />
        </Field>
        <Field id={codeId} label="Setup code" hint={CODE_HINT} error={shownCodeError}>
          <Input
            id={codeId}
            size="lg"
            mono
            autoComplete="off"
            spellCheck={false}
            value={code}
            onChange={(e) => {
              setCode(e.target.value)
              if (codeError) setCodeError(null)
            }}
            {...fieldAria(codeId, true, shownCodeError)}
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
          Create account
        </Button>
      </form>
    </AuthLayout>
  )
}
