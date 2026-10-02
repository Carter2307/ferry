import type { ComponentProps } from 'react';
import logo from '@/public/logo.svg';

/** The Ferry logo — the same image as the web client's `FerryLogo` (web/src/assets/ferry-logo.svg), a little transparent in dark mode. */
export function FerryLogo({ className, ...props }: ComponentProps<'img'>) {
  return (
    <img
      src={logo.src}
      alt=""
      aria-hidden="true"
      className={`opacity-(--logo-opacity) ${className ?? 'size-[18px]'}`}
      {...props}
    />
  );
}
