import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join } from 'node:path';

// The shadcn/radix primitives in @qrtz/ui (Modal, Menu, Tabs, Switch, Select, Slider): one screenshot per
// `primitives-*` story, opened the way a user would (menus and selects are clicked open), plus an axe pass with
// no serious or critical violations.
type Entry = { id: string; type: string };
const index = JSON.parse(
  readFileSync(
    join(import.meta.dirname, '..', 'storybook-static', 'index.json'),
    'utf8',
  ),
) as { entries: Record<string, Entry> };
const stories = Object.values(index.entries).filter(
  (e) => e.type === 'story' && e.id.startsWith('primitives-'),
);
const axeSource = readFileSync(
  createRequire(import.meta.url).resolve('axe-core/axe.min.js'),
  'utf8',
);

for (const s of stories) {
  test(s.id, async ({ page }, info) => {
    await page.goto(`/iframe.html?id=${s.id}&viewMode=story`);
    await page.locator('#storybook-root > *').first().waitFor();
    await page.waitForLoadState('networkidle');
    await page.evaluate(() => document.fonts.ready);
    const trigger = page
      .locator('[aria-haspopup="menu"], [role="combobox"]')
      .first();
    if (await trigger.count()) {
      await trigger.click();
      await page.locator('[role="menu"], [role="listbox"]').first().waitFor();
    }
    await page.screenshot({ path: info.outputPath(`${s.id}.png`) });
    await expect(page).toHaveScreenshot(`${s.id}.png`, {
      maxDiffPixels: 1500,
      threshold: 0.2,
    });
    await page.addScriptTag({ content: axeSource });
    const violations = await page.evaluate(async () => {
      const axe = (
        window as unknown as {
          axe: {
            run: (
              c?: unknown,
              o?: unknown,
            ) => Promise<{
              violations: { id: string; impact: string; nodes: unknown[] }[];
            }>;
          };
        }
      ).axe;
      const r = await axe.run(document, {
        rules: { region: { enabled: false } },
      });
      return r.violations
        .filter((v) => v.impact === 'serious' || v.impact === 'critical')
        .map((v) => `${v.id} (${v.nodes.length})`);
    });
    expect(violations).toEqual([]);
  });
}
