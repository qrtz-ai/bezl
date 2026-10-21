// Storybook ships Nunito Sans for its own chrome; qrtz pages loads only Geist Pixel Square, so drop it from the build.
import fs from 'node:fs';
import path from 'node:path';

const out = path.resolve(import.meta.dirname, '..', 'storybook-static');
const walk = (d) =>
  fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(d, e.name);
    return e.isDirectory() ? walk(p) : [p];
  });
for (const f of walk(out)) {
  if (/nunito-sans.*\.woff2$/.test(f)) fs.rmSync(f);
  else if (/\.(html|css)$/.test(f)) {
    const s = fs.readFileSync(f, 'utf8');
    let t = s.replace(/@font-face\s*\{[^}]*Nunito Sans[^}]*\}/g, '');
    if (f.endsWith('index.html'))
      t = t.replace(/<title>[^<]*<\/title>/, '<title>qrtz pages</title>');
    if (t !== s) fs.writeFileSync(f, t);
  }
}
