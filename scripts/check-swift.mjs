// Mac-only Swift can't be compiled on Linux. This catches the cheap compile breakers statically:
// a type declared twice in the qrtz-helper module, and unbalanced #if/#endif in a file.
import fs from 'node:fs';
import path from 'node:path';

const root = path.join(
  import.meta.dirname,
  '..',
  'native',
  'qrtz-helper',
  'Sources',
);
const problems = [];
for (const mod of fs.readdirSync(root)) {
  const seen = new Map();
  const walk = (d) =>
    fs
      .readdirSync(d, { withFileTypes: true })
      .flatMap((e) =>
        e.isDirectory()
          ? walk(path.join(d, e.name))
          : e.name.endsWith('.swift')
            ? [path.join(d, e.name)]
            : [],
      );
  for (const f of walk(path.join(root, mod))) {
    const src = fs.readFileSync(f, 'utf8');
    const ifs = (src.match(/^\s*#if\b/gm) ?? []).length;
    const ends = (src.match(/^\s*#endif\b/gm) ?? []).length;
    if (ifs !== ends) problems.push(`${f}: ${ifs} #if vs ${ends} #endif`);
    // Linux-only and mac-only files may legitimately declare the same type under opposite #if branches.
    const platform = /^#if !os\(macOS\)/m.test(src)
      ? 'linux'
      : /^#if os\(macOS\)/m.test(src)
        ? 'mac'
        : 'all';
    for (const m of src.matchAll(
      /^(?:public |final |private |fileprivate |internal |@\w+(?:\([^)]*\))? )*(?:class|struct|enum|actor|protocol) (\w+)/gm,
    )) {
      const key = m[1];
      for (const p of platform === 'all' ? ['mac', 'linux'] : [platform]) {
        const k = `${p}:${key}`;
        if (seen.has(k))
          problems.push(`${mod}: ${key} declared in ${seen.get(k)} and ${f}`);
        else seen.set(k, f);
      }
    }
  }
}
if (problems.length) {
  console.error('Swift source check failed:\n' + problems.join('\n'));
  process.exit(1);
}
console.log('Swift sources OK');
