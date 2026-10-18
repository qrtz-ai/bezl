# Scoring for the Echo Linux cross-check: the normalisation (close to qrtz-talk::bench::normalize: lowercase, no
# punctuation, number words 0-99 as digits) and the word edit distance. Used by run.py and compare.py.
import re
NUM={w:i for i,w in enumerate("zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen".split())}
for i,w in enumerate("twenty thirty forty fifty sixty seventy eighty ninety".split()): NUM[w]=20+10*i
def norm(s):
    s=s.lower().replace("%"," percent").replace("-"," ")
    w=re.findall(r"[a-z0-9']+",s); o=[]
    for x in w:
        x=x.strip("'")
        if not x: continue
        if x in NUM and o and o[-1].isdigit() and int(o[-1])%10==0 and int(o[-1])>=20 and NUM[x]<10: o[-1]=str(int(o[-1])+NUM[x]); continue
        o.append(str(NUM[x]) if x in NUM else x)
    return o
def ed(r,h):
    D=list(range(len(h)+1))
    for i in range(1,len(r)+1):
        prev=D[0]; D[0]=i
        for j in range(1,len(h)+1):
            cur=D[j]; D[j]=min(D[j]+1,D[j-1]+1,prev+(r[i-1]!=h[j-1])); prev=cur
    return D[-1]
