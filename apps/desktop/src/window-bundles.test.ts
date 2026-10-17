import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { build, type Rolldown } from 'vite';

// ADR 0090: the HUD and the talk pill are their own pages. Their bundles must not pull in the shell, its
// router or any sub app, and every page carries the same dev CSP.
const root = join(dirname(fileURLToPath(import.meta.url)), '..');

type Chunk = Rolldown.OutputChunk;

/** Every chunk a page loads before it can render: the entry and its static imports. */
function closure(
  chunks: Map<string, Chunk>,
  entry: Chunk,
  lazy = false,
): Chunk[] {
  const seen = new Map<string, Chunk>();
  const walk = (c: Chunk) => {
    if (seen.has(c.fileName)) return;
    seen.set(c.fileName, c);
    for (const i of lazy ? [...c.imports, ...c.dynamicImports] : c.imports)
      walk(chunks.get(i)!);
  };
  walk(entry);
  return [...seen.values()];
}

const SHELL_ONLY =
  /packages\/features\/|src\/shell\/(App|SubAppHost|subapps|Palette|Onboarding)\.tsx?$|src\/renderer\.tsx$|node_modules\/react-router/;

describe('window pages', () => {
  it('the hud and pill bundles include no shell, router or sub app modules', async () => {
    // A production build, as `vite build` makes it (vitest sets NODE_ENV=test, which keeps DEV-only code).
    const env = process.env.NODE_ENV;
    process.env.NODE_ENV = 'production';
    const out = (await build({
      root,
      mode: 'production',
      configFile: join(root, 'vite.config.mts'),
      logLevel: 'silent',
      build: { write: false },
    }).finally(() => {
      process.env.NODE_ENV = env;
    })) as Rolldown.RolldownOutput;
    const chunks = new Map(
      out.output
        .filter((o): o is Chunk => o.type === 'chunk')
        .map((c) => [c.fileName, c]),
    );
    const entry = (name: string) =>
      [...chunks.values()].find((c) => c.isEntry && c.name === name)!;
    const sizes: Record<string, number> = {};
    for (const name of ['hud', 'pill', 'main']) {
      const loaded = closure(chunks, entry(name));
      sizes[name] = loaded.reduce((n, c) => n + c.code.length, 0);
      if (name === 'main') continue;
      const modules = loaded.flatMap((c) => c.moduleIds);
      expect(modules.some((m) => /src\/(hud|pill)-main\.tsx$/.test(m))).toBe(
        true,
      );
      expect(modules.filter((m) => SHELL_ONLY.test(m))).toEqual([]);
      // Nor can it lazily reach a sub app.
      expect(loaded.flatMap((c) => c.dynamicImports)).toEqual([]);
    }
    // The main page still reaches the shell and its sub apps (lazily).
    const main = closure(chunks, entry('main'), true);
    expect(
      main.flatMap((c) => c.moduleIds).some((m) => /src\/shell\/App/.test(m)),
    ).toBe(true);
    expect(
      main
        .flatMap((c) => c.moduleIds)
        .some((m) => /packages\/features\/note\//.test(m)),
    ).toBe(true);
    console.info('[window-bundles] js bytes loaded per page', sizes);
  }, 60_000);

  it('hud.html and pill.html carry the same CSP as index.html', () => {
    const csp = (f: string) =>
      /http-equiv="Content-Security-Policy"\s+content="([^"]+)"/.exec(
        readFileSync(join(root, f), 'utf8'),
      )?.[1];
    expect(csp('index.html')).toBeTruthy();
    expect(csp('hud.html')).toBe(csp('index.html'));
    expect(csp('pill.html')).toBe(csp('index.html'));
  });
});
