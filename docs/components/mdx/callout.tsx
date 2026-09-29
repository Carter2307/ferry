import { CircleAlert, CircleCheck, Info, Lightbulb, TriangleAlert } from 'lucide-react';
import type { ComponentProps, ReactNode } from 'react';
import { cn } from '@/lib/cn';

export type CalloutType = 'info' | 'note' | 'tip' | 'idea' | 'success' | 'warning' | 'warn' | 'error' | 'danger';

const TONES = {
  info: { icon: Info, box: 'border-info-border bg-info-soft', ink: 'text-info' },
  tip: { icon: Lightbulb, box: 'border-border-strong bg-surface-200', ink: 'text-primary' },
  success: { icon: CircleCheck, box: 'border-success/30 bg-success-soft', ink: 'text-success' },
  warning: { icon: TriangleAlert, box: 'border-warning-border bg-warning-soft', ink: 'text-warning' },
  danger: { icon: CircleAlert, box: 'border-destructive-border bg-destructive-soft', ink: 'text-destructive' },
} as const;

function tone(type: CalloutType): keyof typeof TONES {
  switch (type) {
    case 'warn':
    case 'warning':
      return 'warning';
    case 'error':
    case 'danger':
      return 'danger';
    case 'tip':
    case 'idea':
      return 'tip';
    case 'success':
      return 'success';
    default:
      return 'info';
  }
}

/**
 * Admonition in the Ferry tones (Supabase-style: soft fill, tone border, 8px radius).
 *
 * ```mdx
 * <Callout type="warning" title="Back up the data directory">…</Callout>
 * ```
 * type: info (default) · note · tip · idea · success · warning (warn) · danger (error)
 */
export function Callout({
  type = 'info',
  title,
  icon,
  className,
  children,
  ...props
}: Omit<ComponentProps<'div'>, 'title'> & { type?: CalloutType; title?: ReactNode; icon?: ReactNode }) {
  const t = TONES[tone(type)];
  const Icon = t.icon;
  return (
    <div
      role={tone(type) === 'danger' || tone(type) === 'warning' ? 'note' : undefined}
      data-callout={tone(type)}
      className={cn('my-5 flex gap-3 rounded-[var(--ferry-radius-lg)] border px-4 py-3 text-[14px] leading-relaxed', t.box, className)}
      {...props}
    >
      <span className={cn('mt-[3px] shrink-0 [&_svg]:size-4', t.ink)} aria-hidden="true">
        {icon ?? <Icon />}
      </span>
      <div className="min-w-0 flex-1">
        {title ? <p className="my-0! mb-1! font-medium text-foreground">{title}</p> : null}
        <div className="prose-no-margin text-foreground-light [&_p]:my-1.5 [&_pre]:my-2">{children}</div>
      </div>
    </div>
  );
}
