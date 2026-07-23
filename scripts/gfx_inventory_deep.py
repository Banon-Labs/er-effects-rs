#!/usr/bin/env python3
"""Recursive GFX tag-walker: descends into DefineSprite(39) bodies so nested
control tags (PlaceObject2/3, ShowFrame, FrameLabel, RemoveObject2) are counted,
matching ffdec swf2xml recursion. Reports both top-level and deep tallies and
prints an ordered deep tag list for a single file when given as argv[2].
"""
import sys, os, glob, struct, json

TAGN = {0:"End",1:"ShowFrame",2:"DefineShape",9:"SetBackgroundColor",22:"DefineShape2",
 26:"PlaceObject2",28:"RemoveObject2",32:"DefineShape3",37:"DefineEditText",39:"DefineSprite",
 43:"FrameLabel",69:"FileAttributes",70:"PlaceObject3",71:"ImportAssets2",74:"CSMTextSettings",
 75:"DefineFont3",76:"SymbolClass",77:"Metadata",78:"DefineScalingGrid",82:"DoABC",83:"DefineShape4",
 86:"DefineSceneAndFrameLabelData",1000:"GFX_ExporterInfo",1009:"GFX_DefineExternalImage2"}

def name(c): return TAGN.get(c, "UNKNOWN-%d" % c)

class Bits:
    def __init__(s, d, p): s.d=d; s.p=p; s.b=0
    def ub(s, n):
        v=0
        for _ in range(n):
            bit=(s.d[s.p]>>(7-s.b))&1; v=(v<<1)|bit; s.b+=1
            if s.b==8: s.b=0; s.p+=1
        return v
    def al(s):
        if s.b: s.b=0; s.p+=1

def header_tagstart(data):
    br=Bits(data,8); nb=br.ub(5)
    for _ in range(4): br.ub(nb)
    br.al()
    return br.p+4   # +frameRate(2)+frameCount(2)

def walk(data, pos, end, deep, counter, top_only_counter=None, depth=0):
    """Walk tag stream [pos,end). If deep, recurse into DefineSprite(39).
    counter[code]+=1 for every tag (deep). top_only_counter counts depth==0 only."""
    while pos+2 <= end:
        w=struct.unpack_from("<H",data,pos)[0]; pos+=2
        code=w>>6; ln=w&0x3f
        if ln==0x3f:
            ln=struct.unpack_from("<I",data,pos)[0]; pos+=4
        bs=pos
        counter[code]=counter.get(code,0)+1
        if top_only_counter is not None and depth==0:
            top_only_counter[code]=top_only_counter.get(code,0)+1
        if code==0:
            return pos
        if bs+ln>end:
            return pos
        if deep and code==39:
            # DefineSprite body: u16 spriteId, u16 frameCount, then nested tags.
            inner=bs+4
            walk(data, inner, bs+ln, True, counter, top_only_counter, depth+1)
        pos=bs+ln
    return pos

def main():
    pat="/home/banon/er-extract/nuxe-menu-20260619-170932/menu/*.gfx"
    one = sys.argv[1] if len(sys.argv)>1 else None
    files = [os.path.join(os.path.dirname(pat), one)] if one else sorted(glob.glob(pat))
    deep_total={}; top_total={}
    deep_filecount={}
    for f in files:
        data=open(f,"rb").read()
        ts=header_tagstart(data)
        dc={}; tc={}
        walk(data, ts, len(data), True, dc, tc, 0)
        for c,v in dc.items():
            deep_total[c]=deep_total.get(c,0)+v
            deep_filecount.setdefault(c,set()).add(f)
        for c,v in tc.items():
            top_total[c]=top_total.get(c,0)+v
    print("=== DEEP (recurse into sprites) vs TOP-LEVEL tallies, %d files ===" % len(files))
    print("  code  name                              deep_total  deep_files  top_total")
    for c in sorted(deep_total):
        print("  %5d  %-32s %9d  %9d  %9d" % (
            c, name(c), deep_total[c], len(deep_filecount[c]), top_total.get(c,0)))

if __name__=="__main__":
    main()
