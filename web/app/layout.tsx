import type { ReactNode } from 'react';
import Sidebar from '@/components/Sidebar';
import { BridgeProvider } from '@/lib/useBridge';
import './globals.css';

export const metadata = { title: 'Decision Maker' };

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>
        <BridgeProvider>
          <Sidebar />
          <main>{children}</main>
        </BridgeProvider>
      </body>
    </html>
  );
}
