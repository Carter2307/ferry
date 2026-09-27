import Link from 'fumadocs-core/link';
import { ArrowUpRight } from 'lucide-react';
import type { HTMLAttributes, ReactNode } from 'react';
import { cn } from '@/lib/cn';

/** Grid of <Card>s: 2 columns (1 on phones). `cols={3}` for three. */
export function Cards({ className, cols = 2, ...props }: HTMLAttributes<HTMLDivElement> & { cols?: 2 | 3 }) {
  return (
    <div
      className={cn('not-prose my-6 grid grid-cols-1 gap-3 sm:grid-cols-2', cols === 3 && 'lg:grid-cols-3', className)}
      {...props}
    />
  );
}

export type CardProps = Omit<HTMLAttributes<HTMLElement>, 'title'> & {
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  href?: string;
  external?: boolean;
};

/**
 * A bordered card (8px radius): optional 32px icon box, title, description or
 * children. With `href` the whole card is a link.
 */
export function Card({ icon, title, description, href, external, className, children, ...props }: CardProps) {
  const isExternal = external ?? (href ? /^https?:\/\//.test(href) : false);
  const body = (
    <>
      {icon ? (
        <span className="mb-3 inline-flex size-8 items-center justify-center rounded-[var(--ferry-radius-md)] border border-border-strong bg-surface-200 text-foreground-light [&_svg]:size-4">
          {icon}
        </span>
      ) : null}
      <span className="flex items-center gap-1 text-[14px] font-medium text-foreground">
        {title}
        {isExternal ? <ArrowUpRight className="size-3.5 text-foreground-lighter" aria-hidden="true" /> : null}
      </span>
      {description ? <span className="mt-1 block text-[13.5px] leading-relaxed text-foreground-light">{description}</span> : null}
      {children ? (
        <div className="mt-1 text-[13.5px] leading-relaxed text-foreground-light [&_code]:font-mono [&_code]:text-[12.5px] [&_code]:text-foreground [&_p]:my-0">
          {children}
        </div>
      ) : null}
    </>
  );
  const classes = cn(
    'block rounded-[var(--ferry-radius-lg)] border border-border bg-surface-100 p-4 shadow-card transition-colors',
    href && 'hover:border-border-stronger hover:bg-surface-75',
    className,
  );
  if (href) {
    return (
      <Link href={href} external={isExternal} className={classes} {...props}>
        {body}
      </Link>
    );
  }
  return (
    <div className={classes} {...props}>
      {body}
    </div>
  );
}
