// The orchestrator's HTTP API is proxied (no CORS). The bridge's event stream is NOT:
// the browser reads it directly, because rewrites may buffer Server-Sent Events.
const orchestrator = process.env.ORCHESTRATOR_URL ?? 'http://127.0.0.1:8788';

export default {
  async rewrites() {
    return [{ source: '/api/:path*', destination: `${orchestrator}/:path*` }];
  },
};
