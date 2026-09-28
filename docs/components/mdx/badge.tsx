import type { HTMLAttributes } from 'react';
import { cn } from '@/lib/cn';

const VARIANTS = {
  neutral: 'border-border-strong text-foreground-light',
  primary: 'border-primary/30 bg-primary-soft text-primary',
  success: 'border-success/30 bg-success-soft text-success',
  warning: 'border-warning-border bg-warning-soft text-warning',
  danger: 'border-destructive-border bg-destructive-soft text-destructive',
} as const;

/** Tiny uppercase pill (e.g. <Badge variant="warning">Beta</Badge>). */
export function Badge({
  variant = 'neutral',
  className,
  ...props
}: HTMLAttributes<HTMLSpanElement> & { variant?: keyof typeof VARIANTS }) {
  return (
    <span
      className={cn(
        'not-prose inline-flex h-5 items-center rounded-full border px-2 align-middle font-mono text-[10.5px] leading-none tracking-[0.06em] uppercase',
        VARIANTS[variant],
        className,
      )}
      {...props}
    />
  );
}
