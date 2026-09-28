'use client';
import { useTheme } from 'next-themes';
import { useEffect, useId, useState } from 'react';

/**
 * Resolves a design token to an opaque hex color Mermaid can parse (it only
 * understands hex/rgb/hsl, our tokens are oklch and some are translucent):
 * paint it on a 1×1 canvas over the page background and read the pixel back.
 */
function token(name: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback;
  try {
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 1;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    if (!ctx) return fallback;
    const style = getComputedStyle(document.documentElement);
    const bg = style.getPropertyValue('--background').trim();
    const value = style.getPropertyValue(name).trim();
    if (!value) return fallback;
    ctx.fillStyle = bg || '#ffffff';
    ctx.fillRect(0, 0, 1, 1);
    ctx.fillStyle = value;
    ctx.fillRect(0, 0, 1, 1);
    const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
    return `#${[r, g, b].map((c) => (c ?? 0).toString(16).padStart(2, '0')).join('')}`;
  } catch {
    return fallback;
  }
}

/**
 * A Mermaid diagram rendered in the browser with the Ferry tokens (light/dark).
 * Write ```mermaid fences in MDX — they are turned into <Mermaid chart="…" />.
 */
export function Mermaid({ chart }: { chart: string }) {
  const id = useId().replace(/[^a-zA-Z0-9]/g, '');
  const { resolvedTheme } = useTheme();
  const [svg, setSvg] = useState<string>('');
  const [error, setError] = useState<string>('');

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const { default: mermaid } = await import('mermaid');
      const dark = resolvedTheme === 'dark';
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: 'base',
        fontFamily: 'Inter Variable, Inter, system-ui, sans-serif',
        flowchart: { nodeSpacing: 28, rankSpacing: 34, padding: 10, wrappingWidth: 260 },
        sequence: { useMaxWidth: true },
        themeVariables: {
          darkMode: dark,
          fontSize: '14px',
          background: token('--background', dark ? '#131413' : '#fdfdfd'),
          primaryColor: token('--surface-100', dark ? '#181a19' : '#ffffff'),
          primaryTextColor: token('--foreground', dark ? '#edefee' : '#0f0f0f'),
          primaryBorderColor: token('--border-stronger', dark ? '#3a3b3a' : '#c8c8c8'),
          secondaryColor: token('--surface-200', dark ? '#1f2120' : '#f4f4f4'),
          tertiaryColor: token('--surface-75', dark ? '#161716' : '#fafafa'),
          lineColor: token('--foreground-lighter', dark ? '#989a99' : '#6f6f6f'),
          textColor: token('--foreground-light', dark ? '#bcbdbc' : '#4a4a4a'),
          clusterBkg: token('--surface-75', dark ? '#161716' : '#fafafa'),
          clusterBorder: token('--border-strong', dark ? '#2e2f2e' : '#dedede'),
          edgeLabelBackground: token('--background', dark ? '#131413' : '#fdfdfd'),
          noteBkgColor: token('--surface-200', dark ? '#1f2120' : '#f4f4f4'),
          noteBorderColor: token('--border-strong', dark ? '#2e2f2e' : '#dedede'),
          actorBkg: token('--surface-100', dark ? '#181a19' : '#ffffff'),
          actorBorder: token('--border-stronger', dark ? '#3a3b3a' : '#c8c8c8'),
          signalColor: token('--foreground-light', dark ? '#bcbdbc' : '#4a4a4a'),
        },
      });
      try {
        const { svg } = await mermaid.render(`mermaid-${id}-${dark ? 'd' : 'l'}`, chart);
        if (!cancelled) {
          setSvg(svg);
          setError('');
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [chart, id, resolvedTheme]);

  if (error) {
    return (
      <pre className="not-prose my-5 overflow-auto rounded-[var(--ferry-radius-lg)] border border-destructive-border bg-destructive-soft p-4 text-[13px] text-destructive">
        {`Mermaid error: ${error}\n\n${chart}`}
      </pre>
    );
  }
  return (
    <figure
      className="ferry-mermaid not-prose my-6 flex min-h-24 justify-center overflow-x-auto rounded-[var(--ferry-radius-lg)] border border-border bg-surface-75 p-4"
      aria-label="Diagram"
      // Mermaid output (securityLevel "strict": no scripts, no click handlers).
      dangerouslySetInnerHTML={svg ? { __html: svg } : undefined}
    />
  );
}
