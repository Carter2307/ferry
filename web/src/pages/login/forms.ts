/** Shortest password the server accepts. */
export const MIN_PASSWORD_LENGTH = 8

/** What is wrong with a new password, if anything (its length, in characters). */
export function passwordError(password: string): string | null {
  return [...password].length < MIN_PASSWORD_LENGTH ? `Use at least ${MIN_PASSWORD_LENGTH} characters.` : null
}

/** `aria-*` of the input of a `Field` with this id, hint and error. */
export function fieldAria(id: string, hint: unknown, error: string | null | undefined) {
  return {
    'aria-invalid': error ? (true as const) : undefined,
    'aria-describedby': error ? `${id}-error` : hint ? `${id}-hint` : undefined,
  }
}
