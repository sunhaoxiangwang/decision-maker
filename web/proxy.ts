// Guards every request to the Next server, not only /api, so the /api rewrite cannot
// forward anything this check has not seen (no dependence on how a path is matched).
import { NextResponse, type NextRequest } from 'next/server';

const WRITES = new Set(['POST', 'PATCH', 'PUT', 'DELETE']);
const forbidden = () => new NextResponse('Forbidden', { status: 403 });

export function proxy(req: NextRequest) {
  // Only a loopback Host name, on any port: blocks DNS rebinding (a rebound name is not localhost).
  const host = req.headers.get('host') ?? '';
  if (!/^(localhost|127\.0\.0\.1)(:\d+)?$/.test(host)) return forbidden();

  // A browser's Origin must be this same UI. Requests without Origin (curl, page loads) may read.
  const origin = req.headers.get('origin');
  if (origin !== null && origin !== `http://${host}`) return forbidden();

  // Writes must come from this UI and send JSON, which a cross-site HTML form cannot do.
  if (WRITES.has(req.method) && (origin === null || !req.headers.get('content-type')?.startsWith('application/json'))) {
    return forbidden();
  }
  return NextResponse.next();
}

// Everything except Next's static assets, which carry no data.
export const config = { matcher: '/((?!_next/static|_next/image|favicon.ico).*)' };
