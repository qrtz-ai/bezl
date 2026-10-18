import json,glob
order=["whole","s300-c15-o0","s300-c15-o0-min0","s300-c15-o0.5","s300-c15-o1","s200-c15-o0.5","s500-c15-o0.5","s300-c10-o0.5","s300-c20-o0.5","s300-c15-o0.5cap","s300-c15-o1cap","s200-c15-o0","s500-c15-o0","s300-c10-o0","s300-c20-o0","s300-c8-o0","s300-p200a6-c15-o0","s300-p150a6-c15-o0","s300-p200a4-c10-o0","s300-c10-o0.5cap","s300-c6-o0","s300-c6-o0.5cap","s300-c6-o1cap","s300-c15-o0.5r","s300-c15-o1r"]
R={json.load(open(f))["summary"]["name"]:json.load(open(f))["summary"] for f in glob.glob("results/*.json")}
print("| Config | WER | last word missed | segments / cap cuts (mean) | errors within 0.5 s of a cut | audio decoded after release p50 / p90 / max | release CPU ms p50 / p90 / max |")
print("|---|---:|---:|---:|---:|---:|---:|")
for n in order:
  if n not in R: continue
  s=R[n]; a=s["releaseAudioS"]; c=s["releaseCpuMs"]
  print(f"| `{n}` | {s['wer']:.2f} % | {s['lastWordMissedPct']:.1f} % | {s['segmentsMean']} / {s['capCutsMean']} | {s['boundaryErrors']} of {s['errors']} ({s['cuts']} cuts) | {a['p50']} / {a['p90']} / {a['max']} s | {c['p50']:.0f} / {c['p90']:.0f} / {c['max']:.0f} |")
