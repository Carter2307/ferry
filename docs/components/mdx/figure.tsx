'use client';
import { ChevronLeft, ChevronRight, Pause, Play } from 'lucide-react';
import { useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { cn } from '@/lib/cn';

export type Tone = 'default' | 'primary' | 'success' | 'warning' | 'danger' | 'muted';

/** The reader asked the system for less motion: figures then stay still and wait for a click. */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(false);
  useEffect(() => {
    const query = window.matchMedia('(prefers-reduced-motion: reduce)');
    const update = () => setReduced(query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  }, []);
  return reduced;
}

/** True while the element is on screen. */
export function useOnScreen(ref: RefObject<Element | null>, threshold = 0.35): boolean {
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(([entry]) => setVisible(Boolean(entry?.isIntersecting)), { threshold });
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref, threshold]);
  return visible;
}

export type Playback = {
  ref: RefObject<HTMLElement | null>;
  index: number;
  count: number;
  playing: boolean;
  /** Show one step and stop the loop (the reader chose it). */
  select: (index: number) => void;
  toggle: () => void;
  hold: (held: boolean) => void;
};

/**
 * Steps that play in a loop while the figure is on screen. The loop stops
 * while the pointer is on the figure, when the reader chooses a step, and
 * for readers who asked for less motion.
 */
export function usePlayback(count: number, interval = 2600): Playback {
  const ref = useRef<HTMLElement | null>(null);
  const visible = useOnScreen(ref);
  const reduced = useReducedMotion();
  const [index, setIndex] = useState(0);
  const [paused, setPaused] = useState(false);
  const [held, setHeld] = useState(false);
  const playing = visible && !paused && !reduced && count > 1;

  useEffect(() => {
    if (!playing || held) return;
    // The last step stays a little longer before the loop starts again.
    const wait = index === count - 1 ? interval * 1.5 : interval;
    const timer = window.setTimeout(() => setIndex((i) => (i + 1) % count), wait);
    return () => window.clearTimeout(timer);
  }, [playing, held, index, count, interval]);

  return {
    ref,
    index: Math.min(index, Math.max(count - 1, 0)),
    count,
    playing,
    select: (next) => {
      setPaused(true);
      setIndex(((next % count) + count) % count);
    },
    toggle: () => setPaused((p) => !p),
    hold: setHeld,
  };
}

const controlClass =
  'inline-flex size-7 items-center justify-center rounded-[var(--ferry-radius-md)] text-foreground-lighter transition-colors hover:bg-selection hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-[var(--ring)] [&_svg]:size-3.5';

/** Previous / play-pause / next, with the step counter. */
export function PlaybackControls({ playback, label }: { playback: Playback; label: string }) {
  const { index, count, playing, select, toggle } = playback;
  return (
    <div className="-ms-1.5 flex shrink-0 items-center gap-0.5 sm:ms-0" role="group" aria-label={`${label}: steps`}>
      <button type="button" className={controlClass} onClick={() => select(index - 1)} aria-label="Previous step">
        <ChevronLeft aria-hidden="true" />
      </button>
      <button type="button" className={controlClass} onClick={toggle} aria-label={playing ? 'Pause' : 'Play'}>
        {playing ? <Pause aria-hidden="true" /> : <Play aria-hidden="true" />}
      </button>
      <button type="button" className={controlClass} onClick={() => select(index + 1)} aria-label="Next step">
        <ChevronRight aria-hidden="true" />
      </button>
      <span className="ms-1.5 min-w-9 font-mono text-[11.5px] text-foreground-lighter tabular-nums">
        {index + 1} / {count}
      </span>
    </div>
  );
}

/** The frame of every figure: a bordered stage, then an optional footer (caption, controls). */
export function FigureFrame({
  children,
  footer,
  caption,
  className,
  stageClassName,
  frameRef,
  onHold,
}: {
  children: ReactNode;
  footer?: ReactNode;
  caption?: ReactNode;
  className?: string;
  stageClassName?: string;
  frameRef?: RefObject<HTMLElement | null>;
  onHold?: (held: boolean) => void;
}) {
  return (
    <figure
      ref={frameRef}
      className={cn(
        'not-prose my-6 overflow-hidden rounded-[var(--ferry-radius-lg)] border border-border bg-surface-75',
        className,
      )}
      onPointerEnter={onHold ? () => onHold(true) : undefined}
      onPointerLeave={onHold ? () => onHold(false) : undefined}
      onFocus={onHold ? () => onHold(true) : undefined}
      onBlur={onHold ? () => onHold(false) : undefined}
    >
      <div className={cn('overflow-x-auto', stageClassName)}>{children}</div>
      {footer ? (
        <div className="flex min-h-12 flex-col gap-1.5 border-t border-border bg-surface-100 px-4 py-2.5 sm:flex-row sm:items-center sm:justify-between sm:gap-3 sm:py-2">
          {footer}
        </div>
      ) : null}
      {caption ? (
        <figcaption className="border-t border-border bg-surface-100 px-4 py-2.5 text-[13px] leading-relaxed text-foreground-lighter">
          {caption}
        </figcaption>
      ) : null}
    </figure>
  );
}
