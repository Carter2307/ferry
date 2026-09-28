import { ImageResponse } from 'next/og';
import { notFound } from 'next/navigation';
import { source } from '@/lib/source';
import { getPageImageUrl } from '@/lib/shared';

export const revalidate = false;

// Dark theme tokens as hex (Satori has no oklch()): background, surface-100,
// foreground, foreground-light, foreground-lighter, primary.
const C = { bg: '#131413', surface: '#181a19', fg: '#edefee', fgLight: '#bcbdbc', fgLighter: '#989a99', primary: '#185cc4', bright: '#6ea4fb' };

export async function GET(_req: Request, { params }: RouteContext<'/og/docs/[...slug]'>) {
  const { slug } = await params;
  const page = source.getPage(slug.slice(0, -1));
  if (!page) notFound();

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
          <svg width="56" height="56" viewBox="0 0 24 24" fill="none">
            <path d="M4 13.5h16l-2.2 4.2a2 2 0 0 1-1.77 1.07H7.97A2 2 0 0 1 6.2 17.7L4 13.5Z" fill={C.bright} />
            <path d="M7.5 13.5V9.25c0-.69.56-1.25 1.25-1.25h6.5c.69 0 1.25.56 1.25 1.25v4.25" stroke={C.bright} strokeWidth="1.8" strokeLinejoin="round" />
            <path d="M12 8V4.5" stroke={C.bright} strokeWidth="1.8" strokeLinecap="round" />
            <path d="M3 21.25c1.5 0 1.5-.9 3-.9s1.5.9 3 .9 1.5-.9 3-.9 1.5.9 3 .9 1.5-.9 3-.9 1.5.9 3 .9" stroke={C.bright} strokeWidth="1.5" strokeLinecap="round" opacity="0.55" />
          </svg>
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
