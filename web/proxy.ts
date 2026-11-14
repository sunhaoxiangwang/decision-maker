// Guards every request to the Next server, not only /api. The /api rewrite then cannot
// forward anything this check has not seen: the guard does not depend on how a path is
// matched or decoded. The UI is localhost-only, so checking Host everywhere costs nothing.
import { NextResponse, type NextRequest } from 'next/server';

const PORT = process.env.UI_PORT ?? '3000'; // must match the port `next dev` runs on
const UI_ORIGIN = `http://localhost:${PORT}`;
const HOSTS = new Set([`localhost:${PORT}`, `127.0.0.1:${PORT}`]);
const WRITES = new Set(['POST', 'PATCH', 'PUT', 'DELETE']);

export function proxy(req: NextRequest) {
  // Host check blocks DNS rebinding: a rebound name is not localhost.
  if (!HOSTS.has(req.headers.get('host') ?? '')) return new NextResponse('Forbidden', { status: 403 });

  // A browser's Origin must be this UI. Requests without Origin (curl, page loads) are allowed.
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

// Everything except Next's static assets, which carry no data and are served from disk.
export const config = { matcher: '/((?!_next/static|_next/image|favicon.ico).*)' };
