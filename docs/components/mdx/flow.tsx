'use client';
import type { CSSProperties } from 'react';
import { cn } from '@/lib/cn';
import { FigureFrame, PlaybackControls, usePlayback, type Tone } from './figure';
import { figureIcon, type FigureIcon } from './icons';

export type FlowStep = {
  /** Two or three words. */
  title: string;
  /** One short sentence: what happens at this step. */
  text?: string;
  icon?: FigureIcon;
  /** `danger` for the step where things can fail, `success` for the good end. */
  tone?: Tone;
};

const ACTIVE: Record<Tone, string> = {
  default: 'border-primary bg-primary-soft text-primary',
  primary: 'border-primary bg-primary-soft text-primary',
  success: 'border-success bg-success-soft text-success',
  warning: 'border-warning-border bg-warning-soft text-warning',
  danger: 'border-destructive-border bg-destructive-soft text-destructive',
  muted: 'border-border-stronger bg-surface-200 text-foreground-light',
};

/**
 * A sequence that plays by itself: one step is lit at a time and its sentence
 * shows below. Use it for "what happens, in order" (3 to 7 steps).
 *
 * <Flow steps={[{ icon: 'hammer', title: 'Build', text: 'Ferry makes an image.' }, …]} />
 */
export function Flow({ steps, interval, caption }: { steps: FlowStep[]; interval?: number; caption?: string }) {
  const playback = usePlayback(steps.length, interval);
  const current = steps[playback.index];

  return (
    <FigureFrame
      frameRef={playback.ref}
      onHold={playback.hold}
      caption={caption}
      footer={
        <>
          <p className="min-w-0 text-[14px] leading-snug text-foreground-light" aria-live="off">
            <span className="font-medium text-foreground">{current?.title}.</span> {current?.text}
          </p>
          <PlaybackControls playback={playback} label={caption ?? 'Sequence'} />
        </>
      }
    >
      {/* phones: three steps per row; wider screens: one row with a line between the steps */}
      <ol
        className="m-0 grid list-none grid-cols-3 gap-y-5 px-2 py-5 sm:flex sm:min-w-[var(--flow-min)] sm:items-start sm:justify-center sm:px-4 sm:py-6"
        style={{ '--flow-min': `${steps.length * 92}px` } as CSSProperties}
      >
        {steps.map((step, i) => {
          const Icon = figureIcon(step.icon);
          const active = i === playback.index;
          const done = i < playback.index;
          return (
            <li key={step.title} className="relative flex flex-col items-center sm:w-[104px] sm:flex-1">
              {/* the line to the next step */}
              {i < steps.length - 1 ? (
                <span
                  aria-hidden="true"
                  className={cn(
                    'absolute top-5 left-[calc(50%+24px)] hidden h-px w-[calc(100%-48px)] transition-colors duration-500 sm:block',
                    done ? 'bg-primary' : 'bg-border-stronger',
                  )}
                />
              ) : null}
              <button
                type="button"
                onClick={() => playback.select(i)}
                aria-current={active ? 'step' : undefined}
                aria-label={`Step ${i + 1}: ${step.title}`}
                className="group flex flex-col items-center gap-2 rounded-[var(--ferry-radius-md)] px-1 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--ring)]"
              >
                <span
                  className={cn(
                    'flex size-10 items-center justify-center rounded-full border transition-all duration-500 [&_svg]:size-[18px]',
                    active
                      ? cn(ACTIVE[step.tone ?? 'default'], 'scale-110')
                      : done
                        ? 'border-border-stronger bg-surface-100 text-foreground-light'
                        : 'border-border-strong bg-surface-100 text-foreground-muted group-hover:text-foreground-light',
                  )}
                >
                  {Icon ? (
                    <Icon aria-hidden="true" />
                  ) : (
                    <span className="font-mono text-[13px]">{i + 1}</span>
                  )}
                </span>
                <span
                  className={cn(
                    'max-w-[96px] text-center text-[12.5px] leading-tight transition-colors duration-500',
                    active ? 'font-medium text-foreground' : 'text-foreground-lighter',
                  )}
                >
                  {step.title}
                </span>
              </button>
            </li>
          );
        })}
      </ol>
    </FigureFrame>
  );
}
