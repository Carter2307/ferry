/**
 * localStorage access that never throws (private mode, disabled storage,
 * quota errors): reads return null, writes are dropped.
 */
export const safeStorage = {
  get(key: string): string | null {
    try {
      return window.localStorage.getItem(key)
    } catch {
      return null
    }
  },
  set(key: string, value: string): void {
    try {
      window.localStorage.setItem(key, value)
    } catch {
      /* storage unavailable */
    }
  },
  remove(key: string): void {
    try {
      window.localStorage.removeItem(key)
    } catch {
      /* storage unavailable */
    }
  },
}
