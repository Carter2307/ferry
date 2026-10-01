import { describe, expect, it } from 'vitest'

import { fieldAria, passwordError } from './forms'
import { safeNext } from './safeNext'

describe('passwordError', () => {
  it('asks for 8 characters, not bytes', () => {
    expect(passwordError('')).toMatch(/at least 8/)
    expect(passwordError('1234567')).toMatch(/at least 8/)
    expect(passwordError('12345678')).toBeNull()
    expect(passwordError('pâté-crê')).toBeNull()
  })
})

describe('fieldAria', () => {
  it('points at the error first, then at the hint', () => {
    expect(fieldAria('f', true, 'bad')).toEqual({ 'aria-invalid': true, 'aria-describedby': 'f-error' })
    expect(fieldAria('f', true, null)).toEqual({ 'aria-invalid': undefined, 'aria-describedby': 'f-hint' })
    expect(fieldAria('f', null, null)).toEqual({ 'aria-invalid': undefined, 'aria-describedby': undefined })
  })
})

describe('safeNext', () => {
  it('only follows paths of the dashboard', () => {
    expect(safeNext('/services/web?tab=logs')).toBe('/services/web?tab=logs')
    expect(safeNext('/cli-login?id=abc')).toBe('/cli-login?id=abc')
    for (const bad of [null, '', 'https://evil.test', '//evil.test', 'services']) {
      expect(safeNext(bad)).toBe('/services')
    }
  })

  it('never loops through the pages shown before signing in', () => {
    expect(safeNext('/login?next=/x')).toBe('/services')
    expect(safeNext('/setup?code=abc')).toBe('/services')
  })
})
