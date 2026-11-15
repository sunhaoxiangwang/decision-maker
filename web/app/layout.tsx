import type { ReactNode } from 'react';
import localFont from 'next/font/local';
import Sidebar from '@/components/Sidebar';
import { BridgeProvider } from '@/lib/useBridge';
import './globals.css';

const instrumentSans = localFont({ src: './fonts/InstrumentSans.ttf', display: 'swap', variable: '--font-sans' });

export const metadata = { title: 'Decision Maker' };

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className={instrumentSans.variable}>
      <body>
        <BridgeProvider>
          <Sidebar />
          <main>{children}</main>
        </BridgeProvider>
      </body>
    </html>
  );
}
