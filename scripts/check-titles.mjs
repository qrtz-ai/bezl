#!/usr/bin/env node
// Brand rule: all qrtz UI chrome is lowercase: headers, titles, buttons, menu items, labels, dropdown options
// (brand names too), placeholders, toasts, empty states, tray and window titles. Only user content (meeting
// titles, transcripts, notes) keeps its case, and that always arrives through expressions, never literals.
// @qrtz/ui also lowercases chrome props at render time (lowerTitle); this check keeps the source literals
// lowercase so accessible names, window titles and the tray match.
//
// Scanned (AST, not regex) in apps/desktop/src, packages/features/*/src and packages/ui/src (tests skipped):
//   - JSX text (except inside <Kbd>, <Code>, <Mono>)
//   - string literals in JSX children expressions ({busy ? 'Stop' : 'Dictate'})
//   - string literals in UI props (label, title, placeholder, aria-label, hint, tooltip, …)
//   - object properties label/title/placeholder/hint/description (menus, tabs, tray, windows, notifications),
//     sub-app manifest names and dialog filter names
//   - the message passed to toast(…) and React state setters (setMsg(…), setExported(…))
// Shortcut glyphs keep their key (⌘K, ⌘⇧Space). Mark deliberate exceptions (sample user content in the
// gallery) with a `check-titles-ignore` comment on the line or the line above.
// Usage: node scripts/check-titles.mjs [--fix]
import fs from 'node:fs';
import path from 'node:path';
import ts from 'typescript';

const ROOT = path.resolve(
  path.dirname(new URL(import.meta.url).pathname),
  '..',
);
const SKIP = new Set([
  'node_modules',
  'out',
  'dist',
  'dist-types',
  '.vite',
  'test-results',
]);
const UI_ATTRS = new Set([
  'label',
  'title',
  'placeholder',
  'aria-label',
  'aria-description',
  'alt',
  'hint',
  'description',
  'tooltip',
  'footer',
  'confirmLabel',
  'cancelLabel',
  'caption',
  'subtitle',
  'emptyText',
  'status',
]);
const UI_PROPS = new Set([
  'label',
  'title',
  'placeholder',
  'hint',
  'description',
]);
const RAW_TAGS = new Set(['Kbd', 'Code', 'Mono']);

export const hasUpper = (s) =>
  /\p{Lu}/u.test(s.replace(/[⌘⇧⌥⌃]+\s*[\p{L}\d]*/gu, ''));

/** Returns flagged spans: { start, end, line, text, what }. */
export function titleProblems(src, rel) {
  const isTsx = rel.endsWith('.tsx');
  const sf = ts.createSourceFile(
    rel,
    src,
    ts.ScriptTarget.Latest,
    true,
    isTsx ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  const lines = src.split('\n');
  const out = [];
  const ignored = (pos) => {
    const l = sf.getLineAndCharacterOfPosition(pos).line;
    return (
      /check-titles-ignore/.test(lines[l] ?? '') ||
      /check-titles-ignore/.test(lines[l - 1] ?? '')
    );
  };
  const flag = (node, text, what) => {
    const start = node.getStart(sf);
    if (!hasUpper(text) || ignored(start)) return;
    out.push({
      start,
      end: node.getEnd(),
      line: sf.getLineAndCharacterOfPosition(start).line + 1,
      text: text.trim(),
      what,
    });
  };
  // String literals reachable in an expression, without descending into calls, JSX or comparisons.
  const literals = (e, what, intoCalls = false) => {
    const walk = (n) => {
      if (ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n))
        return flag(n, n.text, what);
      if (ts.isTemplateExpression(n)) {
        flag(n.head, n.head.text, what);
        n.templateSpans.forEach((s) => flag(s.literal, s.literal.text, what));
        return;
      }
      if (ts.isCallExpression(n) && intoCalls) return n.arguments.forEach(walk);
      if (
        ts.isCallExpression(n) ||
        ts.isJsxElement(n) ||
        ts.isJsxSelfClosingElement(n) ||
        ts.isJsxFragment(n)
      )
        return;
      if (
        ts.isBinaryExpression(n) &&
        /^(===|!==|==|!=|in|instanceof)$/.test(n.operatorToken.getText(sf))
      )
        return;
      if (ts.isElementAccessExpression(n)) return walk(n.expression);
      if (ts.isTypeOfExpression(n)) return;
      ts.forEachChild(n, walk);
    };
    walk(e);
  };
  const tagName = (el) =>
    ts.isJsxElement(el) ? el.openingElement.tagName.getText(sf) : '';
  const manifest = /[\\/]features[\\/][^\\/]+[\\/]src[\\/]manifest\.ts$/.test(
    rel,
  );
  const visit = (n) => {
    if (ts.isJsxText(n)) {
      if (!RAW_TAGS.has(tagName(n.parent))) flag(n, n.text, 'jsx text');
    } else if (
      ts.isJsxExpression(n) &&
      n.expression &&
      !ts.isJsxAttribute(n.parent)
    ) {
      if (!RAW_TAGS.has(tagName(n.parent)))
        literals(n.expression, 'jsx text', true);
    } else if (
      ts.isJsxAttribute(n) &&
      UI_ATTRS.has(n.name.getText(sf)) &&
      n.initializer
    ) {
      const what = `${n.name.getText(sf)} prop`;
      if (ts.isStringLiteral(n.initializer))
        flag(n.initializer, n.initializer.text, what);
      else if (ts.isJsxExpression(n.initializer) && n.initializer.expression)
        literals(n.initializer.expression, what, true);
    } else if (
      ts.isPropertyAssignment(n) &&
      (ts.isIdentifier(n.name) || ts.isStringLiteral(n.name))
    ) {
      const k = n.name.text;
      const inFilters =
        k === 'name' &&
        ts.isArrayLiteralExpression(n.parent.parent) &&
        ts.isPropertyAssignment(n.parent.parent.parent) &&
        n.parent.parent.parent.name.getText(sf) === 'filters';
      if (UI_PROPS.has(k) || (manifest && k === 'name') || inFilters)
        literals(n.initializer, `${k} property`);
    } else if (
      ts.isCallExpression(n) &&
      n.arguments[0] &&
      (n.expression.getText(sf) === 'toast' ||
        (isTsx &&
          /^set[A-Z]\w*$/.test(n.expression.getText(sf)) &&
          !/^set(Timeout|Interval|Immediate)$/.test(n.expression.getText(sf))))
    ) {
      literals(n.arguments[0], 'toast/status', true);
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return out;
}

export function fixSource(src, problems) {
  let s = src;
  for (const p of [...problems].sort((a, b) => b.start - a.start))
    s =
      s.slice(0, p.start) +
      s.slice(p.start, p.end).toLowerCase() +
      s.slice(p.end);
  return s;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const fix = process.argv.includes('--fix');
  const files = [];
  const walk = (dir) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      if (SKIP.has(e.name) || e.name.startsWith('.')) continue;
      const p = path.join(dir, e.name);
      if (e.isDirectory()) walk(p);
      else if (
        /\.tsx?$/.test(p) &&
        !/\.(test|spec)\.tsx?$/.test(p) &&
        !p.endsWith('.d.ts')
      )
        files.push(p);
    }
  };
  walk(path.join(ROOT, 'apps/desktop/src'));
  walk(path.join(ROOT, 'packages/ui/src'));
  for (const d of fs.readdirSync(path.join(ROOT, 'packages/features')))
    if (fs.existsSync(path.join(ROOT, 'packages/features', d, 'src')))
      walk(path.join(ROOT, 'packages/features', d, 'src'));
  const report = [];
  for (const f of files) {
    const rel = path.relative(ROOT, f);
    const src = fs.readFileSync(f, 'utf8');
    const ps = titleProblems(src, rel);
    if (!ps.length) continue;
    if (fix) fs.writeFileSync(f, fixSource(src, ps));
    report.push(
      ...ps.map((p) => `${rel}:${p.line}: ${p.what} "${p.text.slice(0, 60)}"`),
    );
  }
  if (report.length && !fix) {
    console.error(
      `check-titles: ${report.length} UI string(s) not lowercase:\n  ${report.join('\n  ')}\n(run node scripts/check-titles.mjs --fix, or mark user content with check-titles-ignore)`,
    );
    process.exit(1);
  }
  console.log(
    fix
      ? `check-titles: lowercased ${report.length} string(s)\n  ${report.join('\n  ')}`
      : 'check-titles: ok (all UI strings lowercase)',
  );
}
