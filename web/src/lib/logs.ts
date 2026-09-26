/**
 * Severity of one log line, for colouring. The stream alone is not enough:
 * Docker BuildKit, nginx and Python write normal progress to stderr, so a red
 * stderr would make healthy output look like failures. Only lines that say
 * they are errors / warnings get the destructive / warning ink.
 */

export type LogSeverity = 'error' | 'warning' | null

// A severity word standing alone (not "errors", "error_log", "/error.log"…).
const WORD_BOUNDARY_BEFORE = String.raw`(?:^|[\s\[(<:"'|=-])`
const WORD_BOUNDARY_AFTER = String.raw`(?=$|[\s\]):>"'|,!-])`

const ERROR_RE = new RegExp(
  `${WORD_BOUNDARY_BEFORE}(?:error|err|fatal|panic|critical|crit|emerg|alert|severe|exception)${WORD_BOUNDARY_AFTER}` +
    String.raw`|^Traceback \(most recent call last\)|^panic:|"level"\s*:\s*"(?:error|fatal|panic|critical)"|\blevel=(?:error|fatal|panic|critical)\b`,
  'i',
)
const WARNING_RE = new RegExp(
  `${WORD_BOUNDARY_BEFORE}(?:warn|warning|deprecated)${WORD_BOUNDARY_AFTER}` +
    String.raw`|"level"\s*:\s*"warn(?:ing)?"|\blevel=warn(?:ing)?\b`,
  'i',
)
// Ferry's own `==>` lines: only failures are errors.
const SYSTEM_ERROR_RE = /\b(?:failed|failure|error|crashed|timed out|unhealthy)\b/i

export function logSeverity(stream: string, text: string): LogSeverity {
  if (stream === 'system') return SYSTEM_ERROR_RE.test(text) ? 'error' : null
  if (ERROR_RE.test(text)) return 'error'
  if (WARNING_RE.test(text)) return 'warning'
  return null
}
