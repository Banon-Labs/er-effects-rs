#!/usr/bin/env python3
from __future__ import annotations
import json, shutil, subprocess, sys, time, threading
from pathlib import Path


def pause_for(seconds: float) -> None:
    threading.Event().wait(max(float(seconds), 0.0))

WINDOW_CLASS='steam_app_1245620'

SUBPROCESS_TIMEOUT_SECONDS = 10


def run(args):
    return subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=SUBPROCESS_TIMEOUT_SECONDS)

def find_window():
    hyprctl=shutil.which('hyprctl')
    if not hyprctl:
        return None
    try:
        clients=json.loads(run([hyprctl,'clients','-j']).stdout)
    except Exception:
        return None
    for c in clients if isinstance(clients,list) else []:
        if c.get('class') == WINDOW_CLASS:
            return c
    return None

def focus_window(w):
    hyprctl=shutil.which('hyprctl')
    if not hyprctl or not w:
        return
    ws=w.get('workspace')
    ws_id=ws.get('id') if isinstance(ws,dict) else ws
    addr=w.get('address')
    try:
        if ws_id is not None:
            run([hyprctl,'dispatch','workspace',str(ws_id)])
        if addr:
            run([hyprctl,'dispatch','focuswindow',f'address:{addr}'])
            run([hyprctl,'dispatch','alterzorder',f'top,address:{addr}'])
    except Exception:
        pass

def sane_window(w):
    if not w or w.get('mapped') is False or w.get('hidden') is True:
        return False
    at=w.get('at') or []; size=w.get('size') or []
    if len(at)!=2 or len(size)!=2:
        return False
    x,y=int(at[0]),int(at[1]); sx,sy=int(size[0]),int(size[1])
    return x >= 0 and y >= 0 and sx >= 640 and sy >= 360

def latest_placer_after(art:Path):
    p=art/'hypr-window-placer.jsonl'
    if not p.exists():
        return None
    latest=None
    for line in p.read_text(errors='replace').splitlines():
        try:
            obj=json.loads(line)
        except Exception:
            continue
        if obj.get('event')=='placed' and isinstance(obj.get('after'),dict):
            latest=obj
    return latest

def wait_for_placed_window(art:Path, timeout_s:float=12.0):
    deadline=time.time()+timeout_s
    last_reason='not started'
    while time.time()<deadline:
        placed=latest_placer_after(art)
        w=find_window()
        if placed and sane_window(w):
            after=placed['after']
            if w.get('address') == after.get('address'):
                at=w.get('at') or []; size=w.get('size') or []
                pat=after.get('at') or []; psize=after.get('size') or []
                if len(at)==2 and len(size)==2 and list(map(int,at))==list(map(int,pat)) and list(map(int,size))==list(map(int,psize)):
                    return w, placed
                last_reason=f"current geometry {at} {size} != placer geometry {pat} {psize}"
            else:
                last_reason=f"current address {w.get('address') if w else None} != placer address {after.get('address')}"
        elif placed:
            last_reason=f"placed exists but current window not sane: {w}"
        else:
            last_reason='waiting for hypr-window-placer placed event'
        pause_for(0.05)
    raise SystemExit(f'no stable placed ER window: {last_reason}')

def main():
    art=Path(sys.argv[1]); seconds=float(sys.argv[2]); fps=float(sys.argv[3])
    frames=art/'fast-frames'; frames.mkdir(parents=True, exist_ok=True)
    grim=shutil.which('grim')
    if not grim:
        raise SystemExit('missing grim')
    # Do not freeze the first mapped ER client: Gamescope/XWayland can report transient geometry
    # before our Hyprland placer settles it. Synchronize on the placer proof and then require the
    # current exact ER window address+geometry to match that placed record.
    w, placed=wait_for_placed_window(art)
    focus_window(w)
    pause_for(0.05)
    w2=find_window() or w
    if not sane_window(w2) or w2.get('address') != w.get('address'):
        raise SystemExit(f'placed ER window changed/unsafe after focus: before={w} after={w2}')
    at=w2.get('at') or []; size=w2.get('size') or []
    geom=f'{int(at[0])},{int(at[1])} {int(size[0])}x{int(size[1])}'
    meta={'window_initial':{k:w2.get(k) for k in ('class','at','size','mapped','hidden','focusHistoryID','fullscreen','address','workspace')},'placer_record':placed,'geom':geom,'fps':fps,'seconds':seconds,'frames':[]}
    interval=1.0/max(fps,0.1)
    t0=time.time(); idx=0
    while time.time()-t0 < seconds:
        out=frames/f'frame-{idx:03d}.png'
        r=run([grim,'-g',geom,str(out)])
        meta['frames'].append({'frame':idx,'elapsed':round(time.time()-t0,4),'rc':r.returncode,'stderr':r.stderr.strip(),'path':str(out),'exists':out.exists()})
        idx+=1
        target=t0+idx*interval
        while time.time()<target:
            pause_for(0.01)
    meta['actual_frames']=idx
    (art/'fast-capture.json').write_text(json.dumps(meta,indent=2))
    ffmpeg=shutil.which('ffmpeg')
    if ffmpeg:
        mp4=art/f'fast-{fps:g}fps.mp4'
        r=run([ffmpeg,'-y','-hide_banner','-loglevel','error','-framerate',str(fps),'-i',str(frames/'frame-%03d.png'),'-c:v','libx264','-pix_fmt','yuv420p','-crf','18',str(mp4)])
        (art/'fast-encode.json').write_text(json.dumps({'mp4':str(mp4),'exists':mp4.exists(),'rc':r.returncode,'stderr':r.stderr},indent=2))
    print(json.dumps({'done':True,'frames':idx,'geom':geom,'fps':fps,'seconds':seconds}))

if __name__=='__main__':
    if len(sys.argv) != 4:
        print('usage: fast-er-window-capture.py <artifact-dir> <seconds> <fps>', file=sys.stderr)
        raise SystemExit(2)
    main()
