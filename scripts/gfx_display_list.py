#!/usr/bin/env python3
"""Full structural dump of an uncompressed GFX movie (Elden Ring Scaleform).

Purpose: prove display-list z-order (PlaceObject2/3 depth ordering) and extract
text-field / sprite geometry for the now-loading movies (bd er-effects-rs-jsm).

Parses header RECT, and all tags relevant to display-list z-order analysis:
PlaceObject2/3 (depth/char/name/matrix), DefineSprite (recursing), DefineEditText
(bounds, var name, flags), DefineShape bounds, GFX_DefineExternalImage2,
ImportAssets2, SymbolClass, FrameLabel, ExporterInfo, DefineSceneAndFrameLabelData.

Matrix/RECT decoding mirrors crates/er-gfx/src/codec (SWF bit order, twips).
Usage: python3 scripts/gfx_display_list.py <file.gfx> [--json OUT]
"""
import struct, sys, json

TAGN = {0:"End",1:"ShowFrame",2:"DefineShape",9:"SetBackgroundColor",22:"DefineShape2",
 26:"PlaceObject2",28:"RemoveObject2",32:"DefineShape3",37:"DefineEditText",39:"DefineSprite",
 43:"FrameLabel",56:"ExportAssets",69:"FileAttributes",70:"PlaceObject3",71:"ImportAssets2",
 74:"CSMTextSettings",75:"DefineFont3",76:"SymbolClass",77:"Metadata",78:"DefineScalingGrid",
 82:"DoABC",83:"DefineShape4",86:"DefineSceneAndFrameLabelData",
 1000:"GFX_ExporterInfo",1001:"GFX_DefineExternalImage",1009:"GFX_DefineExternalImage2"}

class Bits:
    def __init__(s,d,p=0): s.d=d; s.p=p; s.b=0
    def ub(s,n):
        v=0
        for _ in range(n):
            v=(v<<1)|((s.d[s.p]>>(7-s.b))&1); s.b+=1
            if s.b==8: s.b=0; s.p+=1
        return v
    def sb(s,n):
        v=s.ub(n)
        if n and v & (1<<(n-1)): v -= 1<<n
        return v
    def align(s):
        if s.b: s.b=0; s.p+=1

def read_rect(bits):
    nb=bits.ub(5)
    xmin=bits.sb(nb); xmax=bits.sb(nb); ymin=bits.sb(nb); ymax=bits.sb(nb)
    bits.align()
    return (xmin,xmax,ymin,ymax)

def read_matrix(bits):
    m={"scale":None,"rotate":None,"tx":0,"ty":0}
    if bits.ub(1):
        n=bits.ub(5); m["scale"]=(bits.sb(n)/65536.0, bits.sb(n)/65536.0)
    if bits.ub(1):
        n=bits.ub(5); m["rotate"]=(bits.sb(n)/65536.0, bits.sb(n)/65536.0)
    n=bits.ub(5)
    m["tx"]=bits.sb(n); m["ty"]=bits.sb(n)   # twips
    bits.align()
    return m

def read_cxform(bits):
    has_add=bits.ub(1); has_mul=bits.ub(1); n=bits.ub(4)
    mul=[bits.sb(n) for _ in range(4)] if has_mul else None
    add=[bits.sb(n) for _ in range(4)] if has_add else None
    bits.align()
    return {"mul":mul,"add":add}

def cstr(d,p):
    e=d.index(b'\0',p)
    return d[p:e].decode('utf-8','replace'), e+1

def parse_place2(body):
    f=body[0]; depth=struct.unpack_from("<H",body,1)[0]; pos=3
    out={"flags":f,"move":bool(f&1),"depth":depth}
    if f&0x80: out["clipactions"]=True; return out
    if f&0x02: out["char"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    if f&0x04:
        b=Bits(body,pos); out["matrix"]=read_matrix(b); pos=b.p
    if f&0x08:
        b=Bits(body,pos); out["cxform"]=read_cxform(b); pos=b.p
    if f&0x10: out["ratio"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    if f&0x20: out["name"],pos=cstr(body,pos)
    if f&0x40: out["clipdepth"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    out["_leftover"]=len(body)-pos
    return out

def parse_place3(body):
    f1=body[0]; f2=body[1]; depth=struct.unpack_from("<H",body,2)[0]; pos=4
    out={"flags1":f1,"flags2":f2,"move":bool(f1&1),"depth":depth}
    if f1&0x80: out["clipactions"]=True; return out
    if f2&0x08: out["classname"],pos=cstr(body,pos)
    if f1&0x02: out["char"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    if f1&0x04:
        b=Bits(body,pos); out["matrix"]=read_matrix(b); pos=b.p
    if f1&0x08:
        b=Bits(body,pos); out["cxform"]=read_cxform(b); pos=b.p
    if f1&0x10: out["ratio"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    if f1&0x20: out["name"],pos=cstr(body,pos)
    if f1&0x40: out["clipdepth"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    if f2&0x01: out["filters"]="present"
    if f2&0x10: out["has_image"]=True
    out["_note"]="po3"
    return out

def parse_edittext(body):
    cid=struct.unpack_from("<H",body,0)[0]
    b=Bits(body,2); bounds=read_rect(b); pos=b.p
    f1=body[pos]; f2=body[pos+1]; pos+=2
    out={"id":cid,"bounds_twips":bounds,"flags1":f1,"flags2":f2,
         "wordwrap":bool(f1&0x40),"multiline":bool(f1&0x20),"readonly":bool(f1&0x08),
         "autosize":bool(f2&0x40),"noselect":bool(f2&0x10),"html":bool(f2&0x02),
         "useoutlines":bool(f2&0x01),"wasstatic":bool(f2&0x04)}
    if f1&0x01: out["font_id"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    if f2&0x80: out["font_class"],pos=cstr(body,pos)
    if (f1&0x01) or (f2&0x80): out["font_height_twips"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    if f1&0x04: out["color_rgba"]=list(body[pos:pos+4]); pos+=4
    if f1&0x02: out["maxlen"]=struct.unpack_from("<H",body,pos)[0]; pos+=2
    if f2&0x20:
        out["layout"]={"align":body[pos],
                       "left":struct.unpack_from("<H",body,pos+1)[0],
                       "right":struct.unpack_from("<H",body,pos+3)[0],
                       "indent":struct.unpack_from("<H",body,pos+5)[0],
                       "leading":struct.unpack_from("<h",body,pos+7)[0]}
        pos+=9
    out["var"],pos=cstr(body,pos)
    if f1&0x80: out["text"],pos=cstr(body,pos)
    return out

def parse_shape(body):
    cid=struct.unpack_from("<H",body,0)[0]
    b=Bits(body,2); bounds=read_rect(b)
    return {"id":cid,"bounds_twips":bounds}

def parse_extimg2(body):
    # GFx DefineExternalImage2: u32 characterId, u16 bitmapFormat, u16 targetW,
    # u16 targetH, then u8-length-prefixed exportName + fileName (observed layout;
    # raw hex retained for verification).
    out={"raw_hex":body.hex()}
    try:
        cid,fmt,w,h=struct.unpack_from("<IHHH",body,0)
        pos=10
        l1=body[pos]; nm1=body[pos+1:pos+1+l1].decode('utf-8','replace'); pos+=1+l1
        l2=body[pos]; nm2=body[pos+1:pos+1+l2].decode('utf-8','replace'); pos+=1+l2
        out.update({"img_char":cid,"bitmap_format":fmt,"target_w":w,"target_h":h,
                    "export_name":nm1,"file_name":nm2,"trailing":body[pos:].hex()})
    except Exception as e:
        out["decode_error"]=str(e)
    return out

def parse_importassets2(body):
    url,pos=cstr(body,0)
    r1,r2=body[pos],body[pos+1]; pos+=2
    n=struct.unpack_from("<H",body,pos)[0]; pos+=2
    syms=[]
    for _ in range(n):
        t=struct.unpack_from("<H",body,pos)[0]; pos+=2
        nm,pos=cstr(body,pos)
        syms.append((t,nm))
    return {"url":url,"reserved":[r1,r2],"symbols":syms}

def parse_symbolclass(body):
    n=struct.unpack_from("<H",body,0)[0]; pos=2
    syms=[]
    for _ in range(n):
        t=struct.unpack_from("<H",body,pos)[0]; pos+=2
        nm,pos=cstr(body,pos)
        syms.append((t,nm))
    return {"symbols":syms}

def parse_exporterinfo(body):
    ver=struct.unpack_from("<H",body,0)[0]
    return {"version_hex":hex(ver),"rest_hex":body[2:].hex()}

def walk(data,pos,end,depth,out,frame_ctr):
    while pos+2<=end:
        w=struct.unpack_from("<H",data,pos)[0]; hpos=pos; pos+=2
        code=w>>6; ln=w&0x3f
        if ln==0x3f: ln=struct.unpack_from("<I",data,pos)[0]; pos+=4
        bs=pos; body=data[bs:bs+ln]
        rec={"off":hpos,"code":code,"tag":TAGN.get(code,f"UNK{code}"),"len":ln,
             "level":depth,"frame":frame_ctr[0]}
        if code==0:
            out.append(rec); return bs+ln
        elif code==26: rec.update(parse_place2(body))
        elif code==70: rec.update(parse_place3(body))
        elif code==28: rec["remove_depth"]=struct.unpack_from("<H",body,0)[0]
        elif code==37: rec.update(parse_edittext(body))
        elif code in (2,22,32,83): rec.update(parse_shape(body))
        elif code==43:
            lbl,_=cstr(body,0); rec["label"]=lbl
        elif code==71: rec.update(parse_importassets2(body))
        elif code==76: rec.update(parse_symbolclass(body))
        elif code==9: rec["rgb"]=list(body)
        elif code==69: rec["attr"]=hex(struct.unpack_from("<I",body,0)[0])
        elif code==1000: rec.update(parse_exporterinfo(body))
        elif code==1009: rec.update(parse_extimg2(body))
        elif code==86: rec["raw_hex"]=body.hex()
        elif code==82: rec["abc_name_guess"]=body[4:64].split(b'\0')[0].decode('utf-8','replace')
        elif code==39:
            sid,fc=struct.unpack_from("<HH",body,0)
            rec["sprite_id"]=sid; rec["sprite_frames"]=fc
            out.append(rec)
            walk(data,bs+4,bs+ln,depth+1,out,[0])
            pos=bs+ln
            continue
        out.append(rec)
        if code==1: frame_ctr[0]+=1
        pos=bs+ln
    return pos

def main():
    path=sys.argv[1]
    data=open(path,'rb').read()
    assert data[:3]==b'GFX', data[:3]
    ver=data[3]; flen=struct.unpack_from("<I",data,4)[0]
    b=Bits(data,8); rect=read_rect(b)
    fr=struct.unpack_from("<H",data,b.p)[0]; fc=struct.unpack_from("<H",data,b.p+2)[0]
    hdr={"version":ver,"file_len":flen,"rect_twips":rect,
         "stage_px":[(rect[1]-rect[0])/20.0,(rect[3]-rect[2])/20.0],
         "frame_rate":fr/256.0,"frame_count":fc}
    out=[]
    walk(data,b.p+4,len(data),0,out,[0])
    doc={"file":path,"header":hdr,"tags":out}
    if len(sys.argv)>3 and sys.argv[2]=="--json":
        json.dump(doc,open(sys.argv[3],"w"),indent=1)
        print("wrote",sys.argv[3])
    else:
        print(json.dumps(doc,indent=1))

if __name__=="__main__":
    main()
