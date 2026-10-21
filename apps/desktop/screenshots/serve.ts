// Serves the built page storybook (storybook-static) for the screenshot run.
import { join, normalize } from 'node:path';

const root = join(import.meta.dir, '..', 'storybook-static');
Bun.serve({
  port: Number(process.argv[2] ?? 6126),
  async fetch(req) {
    const p = normalize(decodeURIComponent(new URL(req.url).pathname));
    const f = Bun.file(join(root, p === '/' ? 'index.html' : p));
    return (await f.exists())
      ? new Response(f)
      : new Response('', { status: 404 });
  },
});
