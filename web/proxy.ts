// Guards the /api rewrite to the orchestrator. Without this, anything that can reach the
// Next server (DNS rebinding, or a cross-site form POST) could reach the orchestrator.
import { NextResponse, type NextRequest } from 'next/server';

const PORT = process.env.UI_PORT ?? '3000'; // must match the port `next dev` runs on
const UI_ORIGIN = `http://localhost:${PORT}`;
const HOSTS = new Set([`localhost:${PORT}`, `127.0.0.1:${PORT}`]);
const WRITES = new Set(['POST', 'PATCH', 'PUT', 'DELETE']);

export function proxy(req: NextRequest) {
  // Host check blocks DNS rebinding: a rebound name is not localhost.
  if (!HOSTS.has(req.headers.get('host') ?? '')) return new NextResponse('Forbidden', { status: 403 });

  // A browser's Origin must be this UI. Requests without Origin (curl) are allowed for reads only.
  const origin = req.headers.get('origin');
  if (origin !== null && origin !== UI_ORIGIN) return new NextResponse('Forbidden', { status: 403 });

  // State-changing requests must come from this UI and send JSON, which a cross-site
  // HTML form cannot do without a preflight that we never allow.
  if (WRITES.has(req.method)) {
    if (origin !== UI_ORIGIN || !req.headers.get('content-type')?.startsWith('application/json')) {
      return new NextResponse('Forbidden', { status: 403 });
    }
  }
  return NextResponse.next();
}

export const config = { matcher: '/api/:path*' };
