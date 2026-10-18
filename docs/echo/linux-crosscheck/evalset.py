# Builds evalset.json: a fixed, seeded subset of LibriSpeech test-clean/test-other (150 each) as (split, id, path, ref).
import glob, os, random, json
out=[]
for split,n in (("test-clean",150),("test-other",150)):
    items=[]
    for t in sorted(glob.glob(f"data/LibriSpeech/{split}/*/*/*.trans.txt")):
        d=os.path.dirname(t)
        for line in open(t):
            i,ref=line.strip().split(" ",1); items.append((split,i,f"{d}/{i}.flac",ref))
    random.Random(7).shuffle(items); out+=items[:n]
json.dump(out,open("evalset.json","w")); print(len(out))
