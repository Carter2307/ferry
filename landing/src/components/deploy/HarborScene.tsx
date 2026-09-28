import { SCENE } from './timeline'

/*
 * A plan view of the harbor, as line art in the design tokens' colors: the
 * proxy gate on land, two berths on piers, a dotted water plane. The drawing is
 * a pure function of the clock `t`.
 *
 *   gate (proxy) ──► junction ──► pier B (new deploy, arrives from the left)
 *                           └───► pier A (live deploy, leaves to the right)
 */
const W = 2400
const CX = 1200
const QUAY_Y = 104
const JUNCTION_Y = 70
const GATE_Y = 12
const TURN_R = 14
const PIER_END = 144
const PIER_HALF = 15
const BERTH = { a: 1320, b: 1080 } as const
const BOAT_BEAM = 54
const BOAT_Y = PIER_END + 2 + BOAT_BEAM / 2
const ROUTE_TOP = -14
const ROUTE_END = BOAT_Y - 4
const B_FROM_X = -240
const A_TO_X = W + 260
/** The part of the harbor on screen; boats come and go past its edges. */
const VIEW = { x: 850, y: -6, w: 700, h: 300 }
/** A tighter crop for phones, so the labels stay readable. */
const VIEW_COMPACT = { x: 962, y: -6, w: 476, h: 290 }

const C = {
  line: 'var(--border-stronger)',
  lineSoft: 'var(--border-strong)',
  ink: 'var(--foreground-muted)',
  text: 'var(--foreground-lighter)',
  panel: 'var(--surface-100)',
  raised: 'var(--surface-300)',
  deck: 'var(--surface-200)',
  primary: 'var(--primary)',
  primarySoft: 'var(--primary-soft)',
  success: 'var(--success)',
  successSoft: 'var(--success-soft)',
  warning: 'var(--warning)',
}

const clamp01 = (x: number) => Math.min(1, Math.max(0, x))
const progress = (t: number, from: number, to: number) => clamp01((t - from) / (to - from))
const easeOut = (x: number) => 1 - (1 - x) ** 3
const easeIn = (x: number) => x ** 3

type Berth = keyof typeof BERTH

/** A request's route: down from the gate, along the quay, down the pier. */
function makeRoute(x: number) {
  const dir = x > CX ? 1 : -1
  const r = TURN_R
  const arc = (Math.PI / 2) * r
  const down = JUNCTION_Y - r - ROUTE_TOP
  const across = Math.abs(x - CX) - 2 * r
  const pier = ROUTE_END - (JUNCTION_Y + r)
  const total = down + arc + across + arc + pier

  const at = (distance: number) => {
    let d = distance
    if (d <= down) return { x: CX, y: ROUTE_TOP + d }
    d -= down
    if (d <= arc) {
      const from = dir > 0 ? Math.PI : 0
      const a = from + (Math.PI / 2 - from) * (d / arc)
      return { x: CX + dir * r + r * Math.cos(a), y: JUNCTION_Y - r + r * Math.sin(a) }
    }
    d -= arc
    if (d <= across) return { x: CX + dir * (r + d), y: JUNCTION_Y }
    d -= across
    if (d <= arc) {
      const to = dir > 0 ? 0 : -Math.PI
      const a = -Math.PI / 2 + (to + Math.PI / 2) * (d / arc)
      return { x: x - dir * r + r * Math.cos(a), y: JUNCTION_Y + r + r * Math.sin(a) }
    }
    return { x, y: JUNCTION_Y + r + Math.min(d - arc, pier) }
  }

  // The drawn branch starts where the trunk ends and stops at the pier's end.
  const branch = [
    `M ${CX} ${JUNCTION_Y - r}`,
    `A ${r} ${r} 0 0 ${dir > 0 ? 0 : 1} ${CX + dir * r} ${JUNCTION_Y}`,
    `H ${x - dir * r}`,
    `A ${r} ${r} 0 0 ${dir > 0 ? 1 : 0} ${x} ${JUNCTION_Y + r}`,
    `V ${PIER_END}`,
  ].join(' ')

  return { total, at, branch }
}

const ROUTES = { a: makeRoute(BERTH.a), b: makeRoute(BERTH.b) }
const TRUNK = `M ${CX} ${ROUTE_TOP} V ${JUNCTION_Y - TURN_R}`

const HULL = 'M -91 -27 H 50 C 74 -27 89 -13 95 0 C 89 13 74 27 50 27 H -91 Q -95 27 -95 23 V -23 Q -95 -27 -91 -27 Z'
const DECK = 'M -87 -21 H 48 C 68 -21 81 -11 86 0 C 81 11 68 21 48 21 H -87 Z'

type Cargo = 'empty' | 'loaded' | 'running' | 'stopped'
type Light = 'off' | 'pending' | 'ok'
type Tint = { fill: string; stroke: string }

const LIVE_A: Tint = { fill: C.primarySoft, stroke: C.primary }
const LIVE_B: Tint = { fill: C.successSoft, stroke: C.success }

function Container({ x, state, tint }: { x: number; state: Cargo; tint: Tint }) {
  if (state === 'empty') return null
  const look =
    state === 'running'
      ? tint
      : state === 'loaded'
        ? { fill: 'transparent', stroke: C.ink }
        : { fill: C.deck, stroke: C.line }
  return (
    <g>
      <rect
        x={x}
        y={-15}
        width={38}
        height={30}
        rx={3}
        className="scene-fill"
        style={{ fill: look.fill, stroke: look.stroke }}
        strokeWidth={1.75}
        strokeDasharray={state === 'loaded' ? '4 3' : undefined}
      />
      {state !== 'loaded' &&
        [1, 2, 3].map((j) => (
          <line
            key={j}
            x1={x + j * 9.5}
            x2={x + j * 9.5}
            y1={-9}
            y2={9}
            className="scene-fill"
            style={{ stroke: look.stroke }}
            strokeOpacity={0.35}
            strokeWidth={1.5}
          />
        ))}
    </g>
  )
}

function Boat({
  x,
  cargo,
  tint,
  wake,
  caption,
}: {
  x: number
  cargo: Cargo[]
  tint: Tint
  wake: number
  caption: string
}) {
  return (
    <g transform={`translate(${x} ${BOAT_Y})`}>
      {wake > 0.02 && (
        <g opacity={wake} stroke={C.line} strokeWidth={2} strokeLinecap="round" fill="none">
          <path d="M -97 -19 L -200 -52" />
          <path d="M -97 19 L -200 52" />
          <path d="M -99 0 H -168" strokeDasharray="4 6" />
        </g>
      )}
      <path d={HULL} fill={C.raised} stroke={C.ink} strokeWidth={1.75} strokeLinejoin="round" />
      <path d={DECK} fill={C.deck} />
      {cargo.map((state, i) => (
        <Container key={i} x={-80 + i * 44} state={state} tint={tint} />
      ))}
      <rect x={30} y={-17} width={30} height={34} rx={4} fill={C.ink} />
      <rect x={53} y={-12} width={3.5} height={24} rx={1.75} fill={C.panel} />
      <text y={60} textAnchor="middle" className="scene-caption" fill={C.text}>
        {caption}
      </text>
    </g>
  )
}

function Beacon({ x, state, t }: { x: number; state: Light; t: number }) {
  const cx = x + PIER_HALF + 16
  const cy = QUAY_Y + 20
  if (state === 'off') return <circle cx={cx} cy={cy} r={6} fill={C.deck} stroke={C.line} strokeWidth={1.5} />
  const fill = state === 'ok' ? C.success : C.warning
  const opacity = state === 'pending' ? 0.45 + 0.55 * (0.5 + 0.5 * Math.sin(t / 95)) : 1
  return (
    <g>
      {state === 'ok' && <circle cx={cx} cy={cy} r={12} fill={C.successSoft} />}
      <circle cx={cx} cy={cy} r={6} fill={fill} opacity={opacity} />
    </g>
  )
}

function Pier({ x }: { x: number }) {
  const l = x - PIER_HALF
  const r = x + PIER_HALF
  return (
    <g>
      <path
        d={`M ${l} ${QUAY_Y - 1} V ${PIER_END} H ${r} V ${QUAY_Y - 1}`}
        fill={C.panel}
        stroke={C.line}
        strokeWidth={1.5}
      />
      <path
        d={`M ${l + 2} ${QUAY_Y + 14} H ${r - 2} M ${l + 2} ${QUAY_Y + 27} H ${r - 2}`}
        stroke={C.lineSoft}
        strokeWidth={1.25}
      />
    </g>
  )
}

export function HarborScene({
  t,
  compact = false,
  className = '',
}: {
  t: number
  compact?: boolean
  className?: string
}) {
  // Boats.
  const bIn = t >= SCENE.boatBEnter
  const bP = progress(t, SCENE.boatBEnter, SCENE.boatBDock)
  const bX = B_FROM_X + (BERTH.b - B_FROM_X) * easeOut(bP)
  const bWake = bIn && bP < 1 ? (1 - bP) ** 2 : 0
  const aP = progress(t, SCENE.boatALeave, SCENE.boatAGone)
  const aX = BERTH.a + (A_TO_X - BERTH.a) * easeIn(aP)
  const aWake = aP > 0 && aP < 1 ? aP ** 2 : 0

  const cargoA: Cargo[] = SCENE.stopA.map((at) => (t < at ? 'running' : 'stopped'))
  const cargoB: Cargo[] = SCENE.loadB.map((at, i) =>
    t < at ? 'empty' : t < (SCENE.startB[i] ?? Infinity) ? 'loaded' : 'running',
  )

  const captionA = t < SCENE.switchAt ? 'Live' : t < SCENE.stopA[1] ? 'Finishing requests' : 'Stopped'
  const captionB =
    t < SCENE.boatBDock
      ? 'Building'
      : t < SCENE.healthStart
        ? 'Starting'
        : t < SCENE.switchAt
          ? 'Checking health'
          : 'Live'

  const lightA: Light = t < SCENE.stopA[0] ? 'ok' : 'off'
  const lightB: Light = t < SCENE.healthStart ? 'off' : t < SCENE.healthy ? 'pending' : 'ok'
  const active: Berth = t < SCENE.switchAt ? 'a' : 'b'
  const pulse = progress(t, SCENE.switchAt, SCENE.switchAt + 650)

  // Requests in flight: each keeps the berth the proxy picked when it came in.
  const dots: { k: number; x: number; y: number }[] = []
  const first = Math.max(0, Math.ceil((t - SCENE.tripMs - SCENE.trafficStart) / SCENE.spawnEvery))
  for (let k = first; ; k++) {
    const spawn = SCENE.trafficStart + k * SCENE.spawnEvery
    if (spawn > t || spawn >= SCENE.trafficEnd) break
    const route = ROUTES[spawn < SCENE.switchAt ? 'a' : 'b']
    const p = (t - spawn) / SCENE.tripMs
    if (p > 1) continue
    dots.push({ k, ...route.at(p * route.total) })
  }

  const view = compact ? VIEW_COMPACT : VIEW
  return (
    <svg
      viewBox={`${view.x} ${view.y} ${view.w} ${view.h}`}
      preserveAspectRatio="xMidYMid meet"
      role="img"
      aria-labelledby="harbor-title harbor-desc"
      className={`block select-none ${className}`}
    >
      <title id="harbor-title">A zero-downtime deploy</title>
      <desc id="harbor-desc">
        The new deploy docks next to the live one. Once its health check passes, the proxy sends new requests to it in
        one step, and the old deploy stops after answering the requests it already had.
      </desc>
      <defs>
        <pattern id="harbor-water" width="16" height="16" patternUnits="userSpaceOnUse">
          <circle cx="8" cy="8" r="1.1" fill={C.line} />
        </pattern>
      </defs>

      {/* Water and the quay edge. */}
      <rect x={0} y={QUAY_Y} width={W} height={600} fill={C.primarySoft} opacity={0.6} />
      <rect x={0} y={QUAY_Y} width={W} height={600} fill="url(#harbor-water)" />
      <line x1={0} x2={W} y1={QUAY_Y} y2={QUAY_Y} stroke={C.line} strokeWidth={1.5} />

      {/* Piers and routes. */}
      <Pier x={BERTH.b} />
      <Pier x={BERTH.a} />
      <g fill="none" strokeLinecap="round" strokeLinejoin="round">
        {(['a', 'b'] as const)
          .filter((b) => b !== active)
          .map((b) => (
            <path key={b} d={ROUTES[b].branch} stroke={C.line} strokeWidth={1.5} strokeDasharray="2 6" />
          ))}
        <path d={ROUTES[active].branch} stroke={C.primary} strokeWidth={2} />
        <path d={TRUNK} stroke={C.primary} strokeWidth={2} />
      </g>
      {pulse > 0 && pulse < 1 && (
        <circle
          cx={CX}
          cy={JUNCTION_Y - TURN_R}
          r={6 + 26 * pulse}
          fill="none"
          stroke={C.primary}
          strokeWidth={1.75}
          opacity={1 - pulse}
        />
      )}

      {/* Requests (under the boats: they board). */}
      <g fill={C.primary}>
        {dots.map((d) => (
          <circle key={d.k} cx={d.x} cy={d.y} r={5} />
        ))}
      </g>

      <Beacon x={BERTH.b} state={lightB} t={t} />
      <Beacon x={BERTH.a} state={lightA} t={t} />

      {t < SCENE.boatAGone && <Boat x={aX} cargo={cargoA} tint={LIVE_A} wake={aWake} caption={captionA} />}
      {bIn && <Boat x={bX} cargo={cargoB} tint={LIVE_B} wake={bWake} caption={captionB} />}

      {/* The proxy: every request comes in through here. */}
      <g>
        <rect
          x={CX - 22}
          y={GATE_Y}
          width={44}
          height={30}
          rx={7}
          fill={C.raised}
          stroke={C.lineSoft}
          strokeWidth={1.5}
        />
        <rect
          x={CX - 7}
          y={GATE_Y + 13}
          width={14}
          height={10.5}
          rx={2}
          fill="none"
          stroke={C.text}
          strokeWidth={1.75}
        />
        <path
          d={`M ${CX - 4.25} ${GATE_Y + 13} V ${GATE_Y + 9.5} a 4.25 4.25 0 0 1 8.5 0 V ${GATE_Y + 13}`}
          fill="none"
          stroke={C.text}
          strokeWidth={1.75}
        />
        <text x={CX + 34} y={GATE_Y + 21} className="scene-label" fill={C.text}>
          my-app.example.com
        </text>
      </g>
    </svg>
  )
}
