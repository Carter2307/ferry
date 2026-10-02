import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { ImageResponse } from 'next/og';
import { notFound } from 'next/navigation';
import { source } from '@/lib/source';
import { getPageImageUrl } from '@/lib/shared';

export const revalidate = false;

// Dark theme tokens as hex (Satori has no oklch()): background, surface-100,
// foreground, foreground-light, foreground-lighter, primary.
const C = { bg: '#131413', surface: '#181a19', fg: '#edefee', fgLight: '#bcbdbc', fgLighter: '#989a99', primary: '#185cc4' };

// The logo as a data URI: Satori draws images from URLs, it has no access to files.
const logo = readFile(path.join(process.cwd(), 'public/logo.svg'), 'base64').then((svg) => `data:image/svg+xml;base64,${svg}`);

export async function GET(_req: Request, { params }: RouteContext<'/og/docs/[...slug]'>) {
  const { slug } = await params;
  const page = source.getPage(slug.slice(0, -1));
  if (!page) notFound();
  const logoSrc = await logo;

  return new ImageResponse(
    (
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          width: '100%',
          height: '100%',
          padding: '72px',
          backgroundColor: C.bg,
          backgroundImage: `radial-gradient(circle, #2a2c2b 1.5px, transparent 1.5px)`,
          backgroundSize: '28px 28px',
          color: C.fg,
          borderBottom: `12px solid ${C.primary}`,
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '18px' }}>
          {/* 772 × 582, a little transparent as everywhere on a dark background (--logo-opacity). */}
          <img src={logoSrc} width={64} height={48} style={{ opacity: 0.85 }} alt="" />
          <span style={{ fontSize: '40px', fontWeight: 600 }}>Ferry</span>
          <span
            style={{
              marginLeft: '8px',
              padding: '4px 16px',
              border: `2px solid #3a3b3a`,
              borderRadius: '999px',
              fontSize: '22px',
              letterSpacing: '0.08em',
              color: C.fgLight,
            }}
          >
            DOCS
          </span>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', marginTop: 'auto' }}>
          <div style={{ fontSize: '68px', fontWeight: 600, lineHeight: 1.1, letterSpacing: '-0.02em' }}>{page.data.title}</div>
          {page.data.description ? (
            <div style={{ marginTop: '24px', fontSize: '30px', lineHeight: 1.4, color: C.fgLighter, maxWidth: '1000px' }}>
              {page.data.description}
            </div>
          ) : null}
        </div>
      </div>
    ),
    { width: 1200, height: 630 },
  );
}

export function generateStaticParams() {
  return source.getPages().map((page) => ({
    lang: page.locale,
    slug: getPageImageUrl(page).segments,
  }));
}
