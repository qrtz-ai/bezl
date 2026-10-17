import { createRequire } from 'node:module';
import fs from 'node:fs';
import type { Page } from '@playwright/test';

/** Serious/critical axe violations (WCAG 2.1 A/AA) on the current view. */
// axe-core is evaluated over CDP (not a <script> tag), so the app's strict CSP stays untouched.
const axeSource = fs.readFileSync(
  createRequire(__filename).resolve('axe-core/axe.min.js'),
  'utf8',
);
export async function axe(win: Page, name: string) {
  await win.evaluate(axeSource);
  const r = await win.evaluate(() =>
    (
      window as unknown as {
        axe: {
          run: (
            c: unknown,
            o: unknown,
          ) => Promise<{
            violations: {
              id: string;
              impact: string;
              help: string;
              nodes: { target: unknown[]; failureSummary?: string }[];
            }[];
          }>;
        };
      }
    ).axe.run(document, {
      runOnly: {
        type: 'tag',
        values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'],
      },
    }),
  );
  const bad = r.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => `${name}: ${v.id} (${v.nodes.length}) ${v.help}`);
  for (const v of r.violations.filter(
    (v) => v.impact === 'serious' || v.impact === 'critical',
  ))
    for (const n of v.nodes)
      console.log(
        name,
        v.id,
        JSON.stringify(n.target),
        n.failureSummary?.replace(/\s+/g, ' '),
      );
  return bad;
}
