'use client';
import SearchDialog from '@/components/search';
import { RootProvider } from 'fumadocs-ui/provider/next';
import { type ReactNode } from 'react';

/** Search (static Orama index) + theme (system default, Light / Dark / System menu — like the web client). */
export function Provider({ children }: { children: ReactNode }) {
  return (
    <RootProvider
      search={{ SearchDialog }}
      theme={{ attribute: 'class', defaultTheme: 'system', enableSystem: true, disableTransitionOnChange: true }}
    >
      {children}
    </RootProvider>
  );
}
