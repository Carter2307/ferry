// The hero demo: one `ferry up --follow` run, in milliseconds from the start.
// The terminal lines are ferry's real output (crates/ferry-cli/src/commands/up.rs,
// crates/ferry-engine/src/pipeline.rs, crates/ferry-build); the harbor scene
// reads the same clock, so both always show the same moment of the deploy.

export const COMMAND = 'ferry up --follow'
export const TYPE_START = 500
export const TYPE_STEP = 45
export const TYPE_END = TYPE_START + COMMAND.length * TYPE_STEP

const DEPLOY_ID = 'dep-q4m8x2k7c1w9v5b3n6ta'

/** plain: CLI output · system: `==>` deploy log · dim: build output · ok / wait: outcome lines */
export type Tone = 'plain' | 'system' | 'dim' | 'ok' | 'wait'

export interface TerminalLine {
  at: number
  tone: Tone
  text: string
}

export const LINES: readonly TerminalLine[] = [
  { at: 1400, tone: 'plain', text: 'Packing /home/you/my-app …' },
  { at: 1650, tone: 'plain', text: 'Packed 38 file(s), 96.2 KiB' },
  { at: 1850, tone: 'plain', text: 'Uploading 96.2 KiB …' },
  { at: 2300, tone: 'plain', text: `Deploy ${DEPLOY_ID} queued (upload), streaming logs…` },
  { at: 2600, tone: 'system', text: `==> Starting deploy ${DEPLOY_ID} (trigger: upload)` },
  { at: 2850, tone: 'system', text: '==> Extracting uploaded source archive' },
  { at: 3100, tone: 'system', text: '==> Detected Node.js runtime' },
  { at: 3350, tone: 'system', text: `==> Building image ferry/my-app:${DEPLOY_ID}` },
  { at: 3750, tone: 'dim', text: 'added 214 packages, and audited 215 packages in 4s' },
  { at: 4200, tone: 'dim', text: '> my-app@1.4.0 build' },
  { at: 4700, tone: 'ok', text: '==> Build successful 🎉' },
  { at: 4950, tone: 'system', text: '==> Starting 2 instance(s)' },
  {
    at: 5450,
    tone: 'wait',
    text: '==> Waiting for 2 instance(s) to become healthy: GET /healthz must answer with a status below 400 (timeout 120s)',
  },
  { at: 7000, tone: 'ok', text: '==> Health check passed' },
  { at: 7300, tone: 'system', text: '==> Routing traffic to the new instance(s)' },
  { at: 7700, tone: 'system', text: '==> Stopping 2 old instance(s)' },
  { at: 9300, tone: 'ok', text: '==> Your service is live 🎉' },
  { at: 9500, tone: 'ok', text: '==> Available at https://my-app.example.com' },
]

/** Moments of the harbor scene. A is the live deploy, B the new one. */
export const SCENE = {
  boatBEnter: 2600,
  boatBDock: 4950,
  /** B's image is built: its containers are loaded, then started. */
  loadB: [3750, 4200],
  startB: [5100, 5300],
  healthStart: 5450,
  healthy: 7000,
  /** The proxy switches in one step: requests arriving after this go to B. */
  switchAt: 7300,
  /** A stops once the requests it already accepted are answered. */
  stopA: [8650, 8800],
  boatALeave: 8950,
  boatAGone: 11200,
  /** Requests: one enters the gate every `spawnEvery` ms and takes `tripMs` to board. */
  trafficStart: -1600,
  trafficEnd: 12400,
  spawnEvery: 200,
  tripMs: 1250,
} as const

/** When everything has settled: the clock stops here. */
export const DONE = SCENE.trafficEnd + SCENE.tripMs + 100

export const typedChars = (t: number) =>
  Math.max(0, Math.min(COMMAND.length, Math.floor((t - TYPE_START) / TYPE_STEP) + 1))

export const visibleLines = (t: number) => LINES.filter((l) => l.at <= t).length

/** Plain-text transcript for screen readers and the no-JS fallback. */
export const TRANSCRIPT = [`$ ${COMMAND}`, ...LINES.map((l) => l.text)].join('\n')
