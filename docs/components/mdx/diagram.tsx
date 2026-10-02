'use client';
import { useId } from 'react';
import { FigureFrame, PlaybackControls, usePlayback, type Tone } from './figure';
import { figureIcon, type FigureIcon } from './icons';

export type DiagramNode = {
  id: string;
  /** [column, row] on the grid, from 0. Halves are allowed: [1.5, 0]. */
  at: [number, number];
  /** At most 15 characters with an icon, 20 without. */
  label: string;
  /** A second, smaller line (at most 18 characters with an icon). */
  note?: string;
  icon?: FigureIcon;
  tone?: Tone;
};

export type DiagramEdge = {
  /** Defaults to `from>to`. */
  id?: string;
  from: string;
  to: string;
  /** One or two words on the line. */
  label?: string;
  /** Dots move along the line: something travels (requests, data, a signal). */
  flow?: boolean;
  dashed?: boolean;
  /** An arrow head at both ends. */
  both?: boolean;
  tone?: Tone;
};

export type DiagramGroup = {
  label: string;
  /** The nodes inside the box: the box is drawn around them. */
  around?: string[];
  /** Or the first and the last cell of the box, as [column, row]. */
  from?: [number, number];
  to?: [number, number];
};

export type DiagramStep = {
  /** One short sentence: what the picture shows at this step. */
  caption: string;
  /** Nodes that are not there yet (or no longer). Their lines go too. */
  hide?: string[];
  /** Edges that carry something at this step (dots move). */
  flow?: string[];
  /** Edges that are not there at this step. */
  cut?: string[];
  tones?: Record<string, Tone>;
  notes?: Record<string, string>;
};

const CELL_W = 180;
const CELL_H = 96;
const NODE_W = 150;
const NODE_H = 52;
const PAD = 14;
/** Room above the first row, for the label of a group. */
const PAD_TOP = 26;
/** Room between a group box and the nodes inside it. */
const GROUP_GAP = 14;

const STROKE: Record<Tone, string> = {
  default: 'var(--border-stronger)',
  primary: 'var(--primary)',
  success: 'var(--success)',
  warning: 'var(--warning)',
  danger: 'var(--destructive)',
  muted: 'var(--border-strong)',
};
const FILL: Record<Tone, string> = {
  default: 'var(--surface-100)',
  primary: 'var(--primary-soft)',
  success: 'var(--success-soft)',
  warning: 'var(--warning-soft)',
  danger: 'var(--destructive-soft)',
  muted: 'var(--surface-200)',
};
const INK: Record<Tone, string> = {
  default: 'var(--foreground-lighter)',
  primary: 'var(--primary)',
  success: 'var(--success)',
  warning: 'var(--warning)',
  danger: 'var(--destructive)',
  muted: 'var(--foreground-muted)',
};
const LINE: Record<Tone, string> = { ...INK, default: 'var(--foreground-muted)', muted: 'var(--border-stronger)' };
const TONES = Object.keys(STROKE) as Tone[];

type Point = { x: number; y: number };

function center(node: DiagramNode): Point {
  return { x: PAD + (node.at[0] + 0.5) * CELL_W, y: PAD_TOP + (node.at[1] + 0.5) * CELL_H };
}

/** The line between two nodes: straight when they share a row or a column, otherwise right-angled with round corners. */
function route(a: DiagramNode, b: DiagramNode): { d: string; mid: Point } {
  const p = center(a);
  const q = center(b);
  const dx = q.x - p.x;
  const dy = q.y - p.y;
  const gap = 5;
  if (Math.abs(dy) < 1) {
    const s = Math.sign(dx) || 1;
    const x1 = p.x + (s * NODE_W) / 2;
    const x2 = q.x - s * (NODE_W / 2 + gap);
    return { d: `M${x1} ${p.y}L${x2} ${q.y}`, mid: { x: (x1 + x2) / 2, y: p.y - 9 } };
  }
  if (Math.abs(dx) < 1) {
    const s = Math.sign(dy);
    const y1 = p.y + (s * NODE_H) / 2;
    const y2 = q.y - s * (NODE_H / 2 + gap);
    return { d: `M${p.x} ${y1}L${q.x} ${y2}`, mid: { x: p.x, y: (y1 + y2) / 2 } };
  }
  const sx = Math.sign(dx);
  const sy = Math.sign(dy);
  const x1 = p.x + (sx * NODE_W) / 2;
  const x2 = q.x - sx * (NODE_W / 2 + gap);
  const xm = (x1 + x2) / 2;
  const r = Math.min(10, Math.abs(dy) / 2, Math.abs(x2 - x1) / 2);
  const d = [
    `M${x1} ${p.y}`,
    `L${xm - sx * r} ${p.y}`,
    `Q${xm} ${p.y} ${xm} ${p.y + sy * r}`,
    `L${xm} ${q.y - sy * r}`,
    `Q${xm} ${q.y} ${xm + sx * r} ${q.y}`,
    `L${x2} ${q.y}`,
  ].join('');
  return { d, mid: { x: xm, y: (p.y + q.y) / 2 } };
}

/**
 * Boxes and arrows on a grid, drawn with the Ferry colors (light and dark).
 * Dots move along the `flow` edges. With `steps`, the picture changes step by
 * step in a loop, with one caption per step.
 *
 * <Diagram
 *   nodes={[{ id: 'browser', at: [0, 0], icon: 'browser', label: 'Browser' }, …]}
 *   edges={[{ from: 'browser', to: 'proxy', flow: true }]}
 * />
 */
export function Diagram({
  nodes,
  edges = [],
  groups = [],
  steps,
  caption,
  interval,
}: {
  nodes: DiagramNode[];
  edges?: DiagramEdge[];
  groups?: DiagramGroup[];
  steps?: DiagramStep[];
  caption?: string;
  interval?: number;
}) {
  const uid = useId().replace(/[^a-zA-Z0-9]/g, '');
  const playback = usePlayback(steps?.length ?? 0, interval ?? 3000);
  const step = steps?.[playback.index];

  const cols = Math.max(...nodes.map((n) => n.at[0]), ...groups.map((g) => g.to?.[0] ?? 0)) + 1;
  const rows = Math.max(...nodes.map((n) => n.at[1]), ...groups.map((g) => g.to?.[1] ?? 0)) + 1;
  const width = cols * CELL_W + PAD * 2;
  const height = rows * CELL_H + PAD_TOP + PAD;
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const hidden = new Set(step?.hide ?? []);
  const cut = new Set(step?.cut ?? []);
  const flowing = step?.flow ? new Set(step.flow) : null;
  const description = [caption, ...(steps?.map((s) => s.caption) ?? [])].filter(Boolean).join(' ');

  return (
    <FigureFrame
      frameRef={playback.ref}
      onHold={steps ? playback.hold : undefined}
      caption={steps ? undefined : caption}
      footer={
        steps ? (
          <>
            <p className="min-w-0 text-[14px] leading-snug text-foreground-light" aria-live="off">
              {step?.caption}
            </p>
            <PlaybackControls playback={playback} label={caption ?? 'Diagram'} />
          </>
        ) : undefined
      }
    >
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width={width}
        height={height}
        role="img"
        aria-label={description || 'Diagram'}
        className="ferry-diagram mx-auto block h-auto max-w-full"
        style={{ minWidth: Math.min(width, 440) }}
      >
        <defs>
          {TONES.map((tone) => (
            <marker
              key={tone}
              id={`${uid}-${tone}`}
              viewBox="0 0 10 10"
              refX="8"
              refY="5"
              markerWidth="7"
              markerHeight="7"
              orient="auto-start-reverse"
            >
              <path d="M1 1.5L8.5 5L1 8.5" fill="none" stroke={LINE[tone]} strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
            </marker>
          ))}
        </defs>

        {groups.map((group) => {
          // The box goes around the named nodes, or around a range of cells.
          const inside = (group.around ?? []).flatMap((id) => (byId.has(id) ? [center(byId.get(id)!)] : []));
          const corners = inside.length
            ? inside
            : [
                { x: PAD + ((group.from?.[0] ?? 0) + 0.5) * CELL_W, y: PAD_TOP + ((group.from?.[1] ?? 0) + 0.5) * CELL_H },
                { x: PAD + ((group.to?.[0] ?? 0) + 0.5) * CELL_W, y: PAD_TOP + ((group.to?.[1] ?? 0) + 0.5) * CELL_H },
              ];
          const x = Math.min(...corners.map((c) => c.x)) - NODE_W / 2 - GROUP_GAP;
          const y = Math.min(...corners.map((c) => c.y)) - NODE_H / 2 - GROUP_GAP - 14;
          const w = Math.max(...corners.map((c) => c.x)) + NODE_W / 2 + GROUP_GAP - x;
          const h = Math.max(...corners.map((c) => c.y)) + NODE_H / 2 + GROUP_GAP - y;
          return (
            <g key={group.label}>
              <rect x={x} y={y} width={w} height={h} rx="10" fill="var(--surface-200)" stroke="var(--border-strong)" strokeDasharray="4 4" />
              <text x={x + 12} y={y + 16} className="ferry-diagram-group">
                {group.label}
              </text>
            </g>
          );
        })}

        {edges.map((edge) => {
          const a = byId.get(edge.from);
          const b = byId.get(edge.to);
          if (!a || !b) return null;
          const id = edge.id ?? `${edge.from}>${edge.to}`;
          const gone = hidden.has(edge.from) || hidden.has(edge.to) || cut.has(id);
          const moving = !gone && (flowing ? flowing.has(id) : Boolean(edge.flow));
          // With steps, the edges that carry nothing at this step step back.
          const quiet = Boolean(flowing) && !moving;
          const tone = edge.tone ?? (moving ? 'primary' : 'default');
          const { d, mid } = route(a, b);
          return (
            <g key={id} className="ferry-diagram-part" style={{ opacity: gone ? 0 : quiet ? 0.45 : 1 }}>
              <path
                d={d}
                fill="none"
                stroke={LINE[tone]}
                strokeWidth="1.5"
                strokeDasharray={edge.dashed ? '5 4' : undefined}
                markerEnd={`url(#${uid}-${tone})`}
                markerStart={edge.both ? `url(#${uid}-${tone})` : undefined}
              />
              {moving
                ? [0, 0.5].map((offset) => (
                    <circle key={offset} r="3.5" fill={LINE[tone]} className="ferry-diagram-dot">
                      <animateMotion dur="1.6s" begin={`${-offset * 1.6}s`} repeatCount="indefinite" path={d} />
                    </circle>
                  ))
                : null}
              {edge.label ? (
                <text x={mid.x} y={mid.y} textAnchor="middle" dominantBaseline="middle" className="ferry-diagram-edge">
                  {edge.label}
                </text>
              ) : null}
            </g>
          );
        })}

        {nodes.map((node) => {
          const { x, y } = center(node);
          const tone = step?.tones?.[node.id] ?? node.tone ?? 'default';
          const note = step?.notes?.[node.id] ?? node.note;
          const Icon = figureIcon(node.icon);
          const left = x - NODE_W / 2;
          const textX = Icon ? left + 38 : x;
          return (
            <g key={node.id} className="ferry-diagram-part" style={{ opacity: hidden.has(node.id) ? 0 : 1 }}>
              <rect
                x={left}
                y={y - NODE_H / 2}
                width={NODE_W}
                height={NODE_H}
                rx="8"
                fill="var(--background)"
              />
              <rect
                className="ferry-diagram-box"
                x={left}
                y={y - NODE_H / 2}
                width={NODE_W}
                height={NODE_H}
                rx="8"
                style={{ fill: FILL[tone], stroke: STROKE[tone] }}
              />
              {Icon ? <Icon x={left + 12} y={y - 9} width={18} height={18} strokeWidth={1.75} style={{ color: INK[tone] }} aria-hidden="true" /> : null}
              <text
                x={textX}
                y={note ? y - 7 : y}
                textAnchor={Icon ? 'start' : 'middle'}
                dominantBaseline="middle"
                className="ferry-diagram-label"
              >
                {node.label}
              </text>
              {note ? (
                <text
                  x={textX}
                  y={y + 10}
                  textAnchor={Icon ? 'start' : 'middle'}
                  dominantBaseline="middle"
                  className="ferry-diagram-note"
                  style={{ fill: tone === 'default' || tone === 'muted' ? undefined : INK[tone] }}
                >
                  {note}
                </text>
              ) : null}
            </g>
          );
        })}
      </svg>
    </FigureFrame>
  );
}
