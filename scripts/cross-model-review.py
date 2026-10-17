#!/usr/bin/env python3
"""Independent architecture critique of the recording lifecycle by other models via OpenRouter (free tier).

Key: $OPENROUTER_API_KEY or the file named by $QRTZ_OPENROUTER_KEY_FILE (default
/home/box/.secrets/openrouter_qrtz.key). The key is never printed. Output: one markdown file per model in
./review-out/ (gitignored). Usage: python3 scripts/cross-model-review.py [model ...]
"""
import json, os, pathlib, sys, urllib.error, urllib.request

MODELS = sys.argv[1:] or ['nvidia/nemotron-3-ultra-550b-a55b:free', 'google/gemma-4-31b-it:free']
FILES = ['docs/recording-lifecycle.md', 'packages/core/src/note/lifecycle.ts', 'apps/desktop/src/main/note-service.ts']
key = os.environ.get('OPENROUTER_API_KEY') or pathlib.Path(
    os.environ.get('QRTZ_OPENROUTER_KEY_FILE', '/home/box/.secrets/openrouter_qrtz.key')).read_text().strip()
body = '\n\n'.join(f'=== {f} ===\n{pathlib.Path(f).read_text()}' for f in FILES)
prompt = ('You are a senior distributed-systems reviewer doing an independent, adversarial architecture review of a macOS '
          'meeting recorder (Electron main + Swift capture helper subprocess + SQLite worker utilityProcess). Find real '
          'defects: races, illegal/missing transitions, non-idempotent steps, data-loss windows, retry storms, state the '
          'renderer cannot observe, crash-recovery holes, doc/code disagreements. Cite functions and exact scenarios. Rank '
          'P0/P1/P2. List design choices you disagree with. No praise, max ~900 words.\n\n' + body)
out = pathlib.Path('review-out'); out.mkdir(exist_ok=True)
for model in MODELS:
    req = urllib.request.Request('https://openrouter.ai/api/v1/chat/completions',
        data=json.dumps({'model': model, 'messages': [{'role': 'user', 'content': prompt}], 'max_tokens': int(os.environ.get('REVIEW_MAX_TOKENS', '16000')), 'usage': {'include': True}}).encode(),
        headers={'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json',
                 'HTTP-Referer': 'https://qrtz.ai', 'X-Title': 'qrtz'})
    try:
        r = json.load(urllib.request.urlopen(req, timeout=600))
        text = r['choices'][0]['message']['content']
        print(model, 'usage', json.dumps(r.get('usage', {})))
    except urllib.error.HTTPError as e:
        text = f'HTTP {e.code}: {e.read()[:300]!r}'
    (out / (model.replace('/', '_').replace(':', '_') + '.md')).write_text(text)
    print('wrote', model)
