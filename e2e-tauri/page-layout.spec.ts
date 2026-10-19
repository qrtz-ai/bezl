import { expect, test, type Page } from '@playwright/test';

// The one page geometry (packages/ui Layout.tsx PAGE): every top-level page puts its content column at the same
// left x and width, to the pixel, whether its content scrolls or not, so switching pages in the sidebar moves
// nothing. Jared 2026-10-19: the qrtz page shifted against the others. The cause was the scrollbar: WebKit ignores
// `scrollbar-gutter: stable` for the styled ::-webkit-scrollbar, so a page that overflowed lost 10px of width and
// its centered column moved. Every scroll root now always reserves its track (globalCss, AppFrame).

const PAGE = {
  maxWidth: 880,
  padTop: 40,
  gutter: 32,
  headerHeight: 40,
  gap: 24,
};

/** Top-level pages: name → hash route and the sub app that shows it (the qrtz page's settings is its own part). */
const PAGES: [string, string, string][] = [
  ['qrtz', '/qrtz', 'marketplace'],
  ['echo', '/talk', 'talk'],
  ['nano', '/note', 'note'],
  ['ctrl', '/ctrl', 'ctrl'],
  ['snap', '/snap', 'snap'],
  ['settings', '/qrtz/settings', 'settings'],
];

type Geo = {
  x: number;
  width: number;
  pageX: number;
  pageTop: number;
  headerTop: number;
  headerHeight: number;
  firstGap: number | null;
  scrollWidth: number;
  scrolls: boolean;
};

/** The content column of the visible page (its header row spans it) and the frame around it. */
const measure = (page: Page, root: string) =>
  page.evaluate((root): Geo => {
    const p = document.querySelector<HTMLElement>(root)!;
    const sr = p.closest<HTMLElement>('[data-scroll-root]')!;
    const h = p.querySelector<HTMLElement>(':scope > [data-page-header]')!;
    const hr = h.getBoundingClientRect();
    const pr = p.getBoundingClientRect();
    const next = h.nextElementSibling?.getBoundingClientRect();
    return {
      x: hr.x,
      width: hr.width,
      pageX: pr.x,
      pageTop: pr.y + sr.scrollTop,
      headerTop: hr.y + sr.scrollTop,
      headerHeight: hr.height,
      firstGap: next ? next.y - hr.bottom : null,
      scrollWidth: sr.clientWidth,
      scrolls: sr.scrollHeight > sr.clientHeight,
    };
  }, root);

/** Adds tall content to the page so its scroll root overflows; returns the remover. */
const overflow = (page: Page, root: string) =>
  page.evaluate((root) => {
    const d = document.createElement('div');
    d.dataset.layoutSpacer = '';
    d.style.height = '3000px';
    document.querySelector(root)!.appendChild(d);
  }, root);
const unoverflow = (page: Page) =>
  page.evaluate(() =>
    document
      .querySelectorAll('[data-layout-spacer]')
      .forEach((e) => e.remove()),
  );

/** Both states of one page: as it is and with scrolling content. */
async function both(page: Page, root: string, name: string) {
  await unoverflow(page);
  const plain = await measure(page, root);
  await overflow(page, root);
  const tall = await measure(page, root);
  await unoverflow(page);
  expect(tall.scrolls, `${name} overflows with the spacer`).toBe(true);
  return { plain, tall };
}

function shape(g: Geo, name: string) {
  expect(g.headerHeight, `${name} header row`).toBe(PAGE.headerHeight);
  expect(g.headerTop - g.pageTop, `${name} top padding`).toBe(PAGE.padTop);
  expect(g.x - g.pageX, `${name} gutter`).toBe(PAGE.gutter);
  expect(g.width, `${name} column`).toBe(
    Math.min(PAGE.maxWidth, g.scrollWidth - 2 * PAGE.gutter),
  );
  if (g.firstGap !== null)
    expect(g.firstGap, `${name} section gap`).toBe(PAGE.gap);
}

for (const size of [
  { width: 1100, height: 720 },
  { width: 1440, height: 900 },
]) {
  test(`every top-level page has the same content column, scrolling or not (${size.width}x${size.height})`, async ({
    page,
  }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await page.setViewportSize(size);
    let ref: Geo | null = null;
    for (const [name, route, subApp] of PAGES) {
      await page.goto(`/?dev=1&seed=1#${route}`);
      const root = `[data-subapp="${subApp}"]:not([hidden]) [data-page]`;
      await expect(page.locator(root).first()).toBeVisible();
      await page.evaluate(() => document.fonts.ready.then(() => undefined));
      const { plain, tall } = await both(page, root, name);
      shape(plain, name);
      ref ??= plain;
      for (const [state, g] of [
        ['as is', plain],
        ['scrolling', tall],
      ] as const) {
        expect(
          { x: g.x, width: g.width, scrollWidth: g.scrollWidth },
          `${name} (${state}) vs qrtz`,
        ).toEqual({ x: ref.x, width: ref.width, scrollWidth: ref.scrollWidth });
      }
    }
    expect(errors).toEqual([]);
  });

  test(`the onboarding page keeps the same column, scrolling or not (${size.width}x${size.height})`, async ({
    page,
  }) => {
    await page.setViewportSize(size);
    await page.goto('/?dev=1&onboarding=1&mockPerms=denied&mockSpeech=missing');
    const root = '[data-testid="onboarding"] [data-page]';
    await expect(page.locator(root).first()).toBeVisible();
    await page.evaluate(() => document.fonts.ready.then(() => undefined));
    const { plain, tall } = await both(page, root, 'onboarding');
    shape(plain, 'onboarding');
    // No sidebar here, so its x is its own frame's; the column itself is the shared one and never moves.
    expect({ x: tall.x, width: tall.width }).toEqual({
      x: plain.x,
      width: plain.width,
    });
  });
}

// Jared 2026-10-19: app rows show a bare icon like the shared model rows (no tile, border or outline), on the
// section label's left edge, with the same size, tone and icon-to-text gap.
test('qrtz page: app rows use the shared rows’ bare icon, size and left edge', async ({
  page,
}) => {
  await page.goto('/?dev=1&seed=1#/qrtz');
  const market = page.locator('[data-subapp="marketplace"]');
  await expect(market.locator('[data-app-row]').first()).toBeVisible();
  await expect(market.getByLabel('shared packages')).toBeVisible();
  const g = await page.evaluate(() => {
    const m = document.querySelector('[data-subapp="marketplace"]')!;
    const box = (e: Element) => e.getBoundingClientRect();
    const look = (svg: Element) => {
      const text = svg.nextElementSibling!;
      return {
        x: box(svg).x,
        size: box(svg).width,
        color: getComputedStyle(svg).color + svg.getAttribute('stroke'),
        gap: box(text).x - box(svg).right,
        tile: (() => {
          const p = svg.parentElement!;
          const s = getComputedStyle(p);
          return `${s.backgroundColor} ${s.borderTopWidth}`;
        })(),
      };
    };
    return {
      apps: [...m.querySelectorAll('[data-testid^="app-icon-"]')].map(look),
      shared: [...m.querySelectorAll('[aria-label="shared packages"] svg')].map(
        look,
      ),
      label: box(m.querySelector('section')!.firstElementChild!).x,
    };
  });
  expect(g.apps.length).toBeGreaterThan(0);
  expect(g.shared.length).toBeGreaterThan(0);
  const ref = g.shared[0]!;
  expect(ref.x).toBe(g.label);
  for (const a of g.apps) {
    expect({ x: a.x, size: a.size, color: a.color, gap: a.gap }).toEqual({
      x: ref.x,
      size: ref.size,
      color: ref.color,
      gap: ref.gap,
    });
    expect(a.tile).toBe('rgba(0, 0, 0, 0) 0px');
  }
});
