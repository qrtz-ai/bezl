#!/usr/bin/env python3
"""QRT-270 CI: fetch echo's Parakeet TDT v2 set exactly as crates/qrtz-app/src/local_models.json pins it (repo,
revision, every file's sha256), into <dest>/parakeet-tdt-v2 with the `.qrtz-verified` marker the helper wants.
Usage: fetch-echo-model.py <dest models dir>. Files already there with the right hash are kept (CI cache)."""
import hashlib, json, os, sys, urllib.request

root = os.path.dirname(os.path.abspath(__file__))
manifest = os.path.join(root, "..", "..", "..", "crates", "qrtz-app", "src", "local_models.json")
data = json.load(open(manifest))
sets = data["sets"] if isinstance(data, dict) and "sets" in data else data
s = next(x for x in (sets if isinstance(sets, list) else sets.values()) if x.get("id") == "echo-asr")
dest = os.path.join(sys.argv[1], s["dir"])

def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""): h.update(b)
    return h.hexdigest()

for f in s["files"]:
    p = os.path.join(dest, f["path"])
    if os.path.exists(p) and sha(p) == f["sha256"]: continue
    os.makedirs(os.path.dirname(p), exist_ok=True)
    url = f"https://huggingface.co/{s['repo']}/resolve/{s['revision']}/{f['remote']}"
    urllib.request.urlretrieve(url, p + ".part")
    got = sha(p + ".part")
    if got != f["sha256"]: sys.exit(f"sha256 mismatch for {f['path']}: {got}")
    os.replace(p + ".part", p)
open(os.path.join(dest, ".qrtz-verified"), "w").write(s["revision"] + "\n")
print(dest)
