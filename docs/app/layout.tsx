import '@fontsource-variable/inter';
import '@fontsource-variable/source-code-pro';
import './global.css';
import type { Metadata, Viewport } from 'next';
import { Provider } from '@/components/provider';
import { appDescription, appName, siteUrl } from '@/lib/shared';

export const metadata: Metadata = {
  metadataBase: new URL(siteUrl),
  title: {
    default: appName,
    template: `%s · ${appName}`,
  },
  description: appDescription,
  applicationName: appName,
  openGraph: {
    type: 'website',
    siteName: appName,
    title: appName,
    description: appDescription,
    images: '/og/docs/image.png',
  },
  twitter: {
    card: 'summary_large_image',
    title: appName,
    description: appDescription,
    images: '/og/docs/image.png',
  },
};

export const viewport: Viewport = {
  themeColor: [
    { media: '(prefers-color-scheme: light)', color: '#fdfdfd' },
    { media: '(prefers-color-scheme: dark)', color: '#131413' },
  ],
};

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className="flex min-h-screen flex-col">
        <Provider>{children}</Provider>
      </body>
    </html>
  );
}
