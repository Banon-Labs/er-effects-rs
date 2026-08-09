#!/usr/bin/env python3
"""Local visual authoring UI for 05_010_ProfileSelect layout/chrome.

This is an editor for the tracked schema and rebuild path. Its canvas is a
Scaleform-coordinate approximation; Elden Ring's runtime remains the final visual oracle.
It never launches or kills Elden Ring/ME3.
"""
from __future__ import annotations

import argparse
import html
import json
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SCHEMA = REPO / "crates/er-gfx/profile_05_010_layout.toml"
REBUILD = REPO / "scripts/rebuild-profile-05-010-layout.sh"
EDITOR_DIR = REPO / "target/pi-local/profile-editor"
CONTROL_FILE = EDITOR_DIR / "control.txt"
STATUS_FILE = EDITOR_DIR / "status.txt"
PROTOCOL_VERSION = 1
FIELD_NAMES = [
    "PlayerName",
    "StaticText_110502",
    "Level",
    "Location",
    "PlayTime",
    "ErStats",
    "ErCharStats",
    "DriveCell_0",
    "DriveCell_1",
    "DriveCell_2",
]
CHROME_NAMES = ["backing", "cursor", "cursor_body"]
MODES = ["load_character", "save_picker", "drive_row"]
MAX_TEXT_FIELD_WIDTH_PX = 1500
MAX_TEXT_FIELD_FONT_HEIGHT_PX = 80
MAX_ABS_POSITION_PX = 2000
VALID_ALIGNMENTS = {"left", "center", "right"}


def strip_comment(line: str) -> str:
    in_string = False
    escaped = False
    for i, ch in enumerate(line):
        if ch == '"' and not escaped:
            in_string = not in_string
        if ch == "#" and not in_string:
            return line[:i]
        escaped = ch == "\\" and not escaped
        if ch != "\\":
            escaped = False
    return line


def parse_value(value: str):
    value = value.strip()
    if value in {"true", "false"}:
        return value == "true"
    if value.startswith('"') and value.endswith('"'):
        return value[1:-1].replace('\\"', '"').replace('\\n', '\n').replace('\\t', '\t').replace('\\\\', '\\')
    if any(c in value for c in ".eE"):
        return float(value)
    return int(value)


def quote_value(value) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str):
        return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n').replace('\t', '\\t') + '"'
    return str(value)


def default_data():
    return {
        "fields": {name: {} for name in FIELD_NAMES},
        "row_chrome": {name: {} for name in CHROME_NAMES},
        "list": {},
    }


def load_schema(path: Path = SCHEMA):
    data = default_data()
    section = None
    for raw in path.read_text().splitlines():
        line = strip_comment(raw).strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            continue
        if "=" not in line or section is None:
            continue
        key, value = [part.strip() for part in line.split("=", 1)]
        parsed = parse_value(value)
        if section.startswith("field."):
            data["fields"][section.removeprefix("field.")][key] = parsed
        elif section.startswith("row_chrome."):
            data["row_chrome"][section.removeprefix("row_chrome.")][key] = parsed
        elif section == "list":
            data["list"][key] = parsed
    return data


def validate_data(data, selected=None):
    errors = []
    if sorted(data.get("fields", {}).keys()) != sorted(FIELD_NAMES):
        errors.append("field set must exactly match the known 05_010 row text fields")
    if sorted(data.get("row_chrome", {}).keys()) != sorted(CHROME_NAMES):
        errors.append("row_chrome set must exactly match the known row chrome objects")
    for name in FIELD_NAMES:
        field = data.get("fields", {}).get(name, {})
        for key in ["x", "y", "width", "font_height", "align"]:
            if key not in field:
                errors.append(f"field.{name}.{key} is missing")
        try:
            x = float(field.get("x", 0))
            y = float(field.get("y", 0))
            width = int(field.get("width", 0))
            font_height = int(field.get("font_height", 0))
        except (TypeError, ValueError):
            errors.append(f"field.{name} has non-numeric geometry")
            continue
        if abs(x) > MAX_ABS_POSITION_PX or abs(y) > MAX_ABS_POSITION_PX:
            errors.append(f"field.{name} position is outside the safe editor range (+/-{MAX_ABS_POSITION_PX}px)")
        if not (0 < width <= MAX_TEXT_FIELD_WIDTH_PX):
            errors.append(f"field.{name}.width must be 1..{MAX_TEXT_FIELD_WIDTH_PX}px")
        if not (0 < font_height <= MAX_TEXT_FIELD_FONT_HEIGHT_PX):
            errors.append(f"field.{name}.font_height must be 1..{MAX_TEXT_FIELD_FONT_HEIGHT_PX}px")
        if field.get("align") not in VALID_ALIGNMENTS:
            errors.append(f"field.{name}.align must be left, center, or right")
    for name in CHROME_NAMES:
        chrome = data.get("row_chrome", {}).get(name, {})
        for key in ["x", "y", "scale_x", "scale_y", "editable"]:
            if key not in chrome:
                errors.append(f"row_chrome.{name}.{key} is missing")
        try:
            x = float(chrome.get("x", 0))
            y = float(chrome.get("y", 0))
            scale_x = float(chrome.get("scale_x", 0))
            scale_y = float(chrome.get("scale_y", 0))
        except (TypeError, ValueError):
            errors.append(f"row_chrome.{name} has non-numeric geometry")
            continue
        if abs(x) > MAX_ABS_POSITION_PX or abs(y) > MAX_ABS_POSITION_PX:
            errors.append(f"row_chrome.{name} position is outside the safe editor range (+/-{MAX_ABS_POSITION_PX}px)")
        if not (0 < scale_x <= 40) or not (0 < scale_y <= 40):
            errors.append(f"row_chrome.{name} scale must be > 0 and <= 40")
    list_data = data.get("list", {})
    if list_data.get("visible_rows") != 10:
        errors.append("list.visible_rows must stay 10")
    for key in ["row_pitch", "top_recycle_rows", "bottom_recycle_rows", "mask_height", "scrollbar_x", "scrollbar_top_y", "scrollbar_track_height"]:
        if key not in list_data:
            errors.append(f"list.{key} is missing")
    for key in ["row_pitch", "mask_height", "scrollbar_track_height"]:
        try:
            if int(list_data.get(key, 0)) <= 0:
                errors.append(f"list.{key} must be positive")
        except (TypeError, ValueError):
            errors.append(f"list.{key} must be numeric")
    if selected is not None:
        kind = selected.get("kind", "field")
        name = selected.get("name", "PlayerName")
        if kind == "field" and name not in FIELD_NAMES:
            errors.append(f"selected field is unknown: {name}")
        elif kind == "chrome" and name not in CHROME_NAMES:
            errors.append(f"selected chrome object is unknown: {name}")
        elif kind not in {"field", "chrome", "list"}:
            errors.append(f"selected kind is unknown: {kind}")
    return errors


def write_schema(data, path: Path = SCHEMA):
    errors = validate_data(data)
    if errors:
        raise ValueError("invalid 05_010 layout:\n" + "\n".join(f"- {e}" for e in errors))
    lines = [
        "# 05_010_ProfileSelect visual editor schema.",
        "# Edited by scripts/profile-05-010-editor.py or by hand.",
        "# The canvas is approximate; in-game Scaleform rendering is the final oracle.",
        "# Icon_0 stays placed and hidden, never unplaced.",
        "",
    ]
    for name in FIELD_NAMES:
        f = data["fields"][name]
        lines.append(f"[field.{name}]")
        for key in ["x", "y", "width", "font_height", "align", "sample_load_character", "sample_save_picker", "sample_drive_row"]:
            lines.append(f"{key} = {quote_value(f.get(key, ''))}")
        lines.append("")
    for name in CHROME_NAMES:
        c = data["row_chrome"][name]
        lines.append(f"[row_chrome.{name}]")
        for key in ["x", "y", "scale_x", "scale_y", "editable", "source"]:
            lines.append(f"{key} = {quote_value(c.get(key, ''))}")
        lines.append("")
    lines.append("[list]")
    for key in ["row_pitch", "visible_rows", "top_recycle_rows", "bottom_recycle_rows", "mask_height", "scrollbar_x", "scrollbar_top_y", "scrollbar_track_height"]:
        lines.append(f"{key} = {quote_value(data['list'][key])}")
    lines.append("")
    path.write_text("\n".join(lines))


rebuild_state = {"running": False, "returncode": None, "output": ""}


def quote_control(value) -> str:
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\"').replace('\n', '\n').replace('\t', '\t') + '"'


def write_runtime_control(payload):
    data = payload["data"]
    selected = payload.get("selected", {"kind": "field", "name": "PlayerName"})
    sequence = int(payload.get("sequence", 0))
    render_mode = payload.get("render_mode", "offline_approximate")
    text_probe = bool(payload.get("text_probe", False))
    EDITOR_DIR.mkdir(parents=True, exist_ok=True)
    lines = [
        f"profile_editor_protocol = {quote_control(PROTOCOL_VERSION)}",
        f"sequence = {quote_control(sequence)}",
        f"render_mode = {quote_control(render_mode)}",
        f"selected_kind = {quote_control(selected.get('kind', 'field'))}",
        f"selected_name = {quote_control(selected.get('name', 'PlayerName'))}",
    ]
    if text_probe:
        lines.append(f"text_probe = {quote_control(True)}")
    for name in FIELD_NAMES:
        field = data["fields"][name]
        for key in ["x", "y", "width", "font_height", "align"]:
            lines.append(f"field.{name}.{key} = {quote_control(field[key])}")
    for name in CHROME_NAMES:
        obj = data["row_chrome"][name]
        for key in ["x", "y", "scale_x", "scale_y", "editable"]:
            lines.append(f"row_chrome.{name}.{key} = {quote_control(obj[key])}")
    for key in ["row_pitch", "visible_rows", "top_recycle_rows", "bottom_recycle_rows", "mask_height", "scrollbar_x", "scrollbar_top_y", "scrollbar_track_height"]:
        lines.append(f"list.{key} = {quote_control(data['list'][key])}")
    tmp = CONTROL_FILE.with_suffix(".txt.tmp")
    tmp.write_text("\n".join(lines) + "\n")
    tmp.replace(CONTROL_FILE)


def parse_status_text(text: str):
    out = {}
    for raw in text.splitlines():
        line = strip_comment(raw).strip()
        if not line or "=" not in line:
            continue
        key, value = [part.strip() for part in line.split("=", 1)]
        out[key] = parse_value(value)
    return out


def runtime_status():
    if not STATUS_FILE.exists():
        return {"connected": False, "status": "no status.txt yet", "path": str(STATUS_FILE)}
    try:
        status = parse_status_text(STATUS_FILE.read_text())
        status["path"] = str(STATUS_FILE)
        return status
    except Exception as e:
        return {"connected": False, "status": "status parse error", "error": str(e), "path": str(STATUS_FILE)}
protocol_sequence = 0


def protocol_quote(value) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    text = str(value)
    safe = text and all(ch.isalnum() or ch in "_-.:" for ch in text)
    if safe:
        return text
    return '"' + text.replace('\\', '\\\\').replace('"', '\\"') + '"'


def protocol_lines(data, sequence: int, render_mode: str, selected: dict, text_probe=False):
    lines = [
        f"profile_editor_protocol = {PROTOCOL_VERSION}",
        f"sequence = {sequence}",
        f"render_mode = {protocol_quote(render_mode)}",
        f"selected_kind = {protocol_quote(selected.get('kind', 'field'))}",
        f"selected_name = {protocol_quote(selected.get('name', 'PlayerName'))}",
    ]
    if text_probe:
        lines.append(f"text_probe = {protocol_quote(True)}")
    for name in FIELD_NAMES:
        f = data["fields"][name]
        for key in ["x", "y", "width", "font_height", "align"]:
            lines.append(f"field.{name}.{key} = {protocol_quote(f[key])}")
    for name in CHROME_NAMES:
        c = data["row_chrome"][name]
        for key in ["x", "y", "scale_x", "scale_y", "editable"]:
            lines.append(f"row_chrome.{name}.{key} = {protocol_quote(c[key])}")
    for key in ["row_pitch", "visible_rows", "top_recycle_rows", "bottom_recycle_rows", "mask_height", "scrollbar_x", "scrollbar_top_y", "scrollbar_track_height"]:
        lines.append(f"list.{key} = {protocol_quote(data['list'][key])}")
    return "\n".join(lines) + "\n"


def write_control(data, render_mode="offline_approximate", selected=None, text_probe=False):
    global protocol_sequence
    selected = selected or {"kind": "field", "name": "PlayerName"}
    protocol_sequence += 1
    EDITOR_DIR.mkdir(parents=True, exist_ok=True)
    CONTROL_FILE.write_text(protocol_lines(data, protocol_sequence, render_mode, selected, text_probe))
    return protocol_sequence


def parse_protocol_file(path: Path):
    if not path.exists():
        return None
    out = {}
    for raw in path.read_text(errors="replace").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line or "=" not in line:
            continue
        key, value = [part.strip() for part in line.split("=", 1)]
        if value.startswith('"') and value.endswith('"'):
            value = value[1:-1].replace('\\"', '"').replace('\\\\', '\\')
        out[key] = value
    return out


def runtime_status():
    status = parse_protocol_file(STATUS_FILE)
    if status is None:
        return {
            "profile_editor_protocol": PROTOCOL_VERSION,
            "ack_sequence": 0,
            "connected": False,
            "status": "disconnected",
            "active_surface": "none",
            "selected_kind": "",
            "selected_name": "",
            "applied_count": 0,
            "unsupported_count": 0,
            "error": "no runtime status file yet; live mode is not connected",
        }
    return {
        "profile_editor_protocol": int(status.get("profile_editor_protocol", PROTOCOL_VERSION)),
        "ack_sequence": int(status.get("ack_sequence", 0)),
        "connected": status.get("connected") == "true",
        "status": status.get("status", "unknown"),
        "active_surface": status.get("active_surface", "unknown"),
        "selected_kind": status.get("selected_kind", ""),
        "selected_name": status.get("selected_name", ""),
        "applied_count": int(status.get("applied_count", 0)),
        "unsupported_count": int(status.get("unsupported_count", 0)),
        "error": status.get("error", ""),
    }



def start_rebuild():
    if rebuild_state["running"]:
        return
    def run():
        rebuild_state.update({"running": True, "returncode": None, "output": ""})
        proc = subprocess.run(
            [str(REBUILD), "--hot-reload"],
            cwd=REPO,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=30,
        )
        rebuild_state.update({"running": False, "returncode": proc.returncode, "output": proc.stdout[-20000:]})
    threading.Thread(target=run, daemon=True).start()


HTML = r"""
<!doctype html>
<meta charset="utf-8">
<title>05_010 ProfileSelect Visual Editor</title>
<style>
:root { color-scheme: dark; font-family: system-ui, sans-serif; }
body { margin:0; display:grid; grid-template-columns: 330px 1fr 360px; height:100vh; background:#151515; color:#ddd; }
aside, section { overflow:auto; border-right:1px solid #333; padding:12px; }
button, input, select { background:#222; color:#eee; border:1px solid #555; border-radius:4px; padding:4px; }
button { cursor:pointer; } button.primary { border-color:#9d7; }
.warn { color:#f0c36a; } .bad { color:#ff7a7a; } .ok { color:#8f8; }
.tree button { display:block; width:100%; margin:3px 0; text-align:left; }
.tree button.sel { background:#38451f; border-color:#9d7; }
.grid { display:grid; grid-template-columns: 120px 1fr; gap:6px; align-items:center; }
.controls { display:flex; gap:6px; flex-wrap:wrap; margin:8px 0; }
pre { background:#0f0f0f; border:1px solid #333; padding:8px; white-space:pre-wrap; max-height:260px; overflow:auto; }
svg { width:100%; height:calc(100vh - 70px); background:#080808; border:1px solid #333; }
.field { fill:#2f72ff33; stroke:#6ca0ff; stroke-width:1; cursor:move; }
.chrome { fill:#d6922933; stroke:#ffbd5a; stroke-width:1; cursor:move; }
.readonly { fill:#7773; stroke:#aaa; stroke-dasharray:4 3; }
.ink { fill:#99ff6640; stroke:#99ff66; stroke-width:.6; pointer-events:none; }
.label { fill:#ddd; font: 12px monospace; pointer-events:none; }
.rowline { stroke:#333; stroke-width:.5; }
</style>
<body>
<aside>
  <h2>05_010 ProfileSelect</h2>
  <p class="warn">Offline canvas is approximate. Live runtime calibration is authoritative only after Elden Ring/Scaleform writes matching acks.</p>
  <label>render mode <select id="renderMode"><option value="offline_approximate">offline approximate canvas (not proof)</option><option value="live_runtime">live runtime calibration (authoritative when connected)</option></select></label>
  <div id="runtimeBadge" class="bad">runtime disconnected</div>
  <div class="controls">
    <button class="primary" id="saveBtn">save schema</button>
    <button id="rebuildBtn">hot rebuild GFX</button>
    <button id="probeTextBtn">probe selected text</button>
    <button id="reloadBtn">reload</button>
  </div>
  <label>sample mode <select id="mode"><option>load_character</option><option>save_picker</option><option>drive_row</option></select></label>
  <div class="tree" id="tree"></div>
</aside>
<section>
  <h3>Approximate layout canvas</h3>
  <svg id="canvas" viewBox="-620 -285 1260 570"></svg>
</section>
<aside>
  <h3 id="title">select object</h3>
  <div id="editor"></div>
  <h3>Runtime bridge status</h3>
  <pre id="runtimeStatus">disconnected</pre>
  <h3>Rebuild output</h3>
  <pre id="log">not run</pre>
  <h3>Files</h3>
  <pre id="paths"></pre>
</aside>
<script>
let data=null, selected={kind:'field', name:'PlayerName'}, drag=null, runtimeStatus=null;
const fields = %FIELDS%;
const chromeObjects = %CHROME%;
const paths = %PATHS%;
document.getElementById('paths').textContent = JSON.stringify(paths, null, 2);
async function load(){ data = await (await fetch('/schema')).json(); renderTree(); renderEditor(); draw(); }
function showError(err){ const msg=(err&&err.stack)||String(err); document.getElementById('log').textContent='EDITOR JS ERROR\n'+msg; console.error(err); }
function objectList(){ return fields.map(n=>['field',n]).concat(chromeObjects.map(n=>['chrome',n]), [['list','list']]); }
function chromeLabel(name){ return ({backing:'normal row backing (Backing/char54)', cursor:'highlight wrapper (Cursor/char75)', cursor_body:'highlight inner body (Cursor/CursorBody/char73)'})[name] || name; }
function renderTree(){ const t=document.getElementById('tree'); t.innerHTML=''; for(const [kind,name] of objectList()){ const b=document.createElement('button'); b.className=(selected.kind==kind&&selected.name==name)?'sel':''; b.textContent=(kind=='field'?'text ':'')+(kind=='chrome'?'chrome '+chromeLabel(name):'')+(kind=='list'?'list':''); b.onclick=()=>{selected={kind,name};renderTree();renderEditor();draw();}; t.appendChild(b);} }
function getObj(){ if(selected.kind=='field') return data.fields[selected.name]; if(selected.kind=='chrome') return data.row_chrome[selected.name]; return data.list; }
function numberInput(obj,k,step=1){ return `<label>${k}</label><input type="number" step="${step}" value="${obj[k]}" onchange="setVal('${k}', this.value)">`; }
function textInput(obj,k){ return `<label>${k}</label><input value="${String(obj[k]??'').replaceAll('&','&amp;').replaceAll('"','&quot;')}" onchange="setText('${k}', this.value)">`; }
function renderEditor(){ const o=getObj(), e=document.getElementById('editor'); document.getElementById('title').textContent=selected.kind+': '+selected.name; let h='<div class="grid">'; if(selected.kind=='field'){ for(const k of ['x','y']) h+=numberInput(o,k,1); h+=`<div></div><div class="ok">x/y nudges are the live-safe field operation. They do not touch text box width or live text scale.</div>`; for(const k of ['width','font_height']) h+=numberInput(o,k,1); h+=`<label>align</label><select onchange="setText('align',this.value)">${['left','center','right'].map(a=>`<option ${o.align==a?'selected':''}>${a}</option>`).join('')}</select>`; h+=`<div></div><div class="warn">width/font/align are asset-level controls only. They rebuild GFX and need a fresh movie/load to prove.</div>`; for(const k of ['sample_load_character','sample_save_picker','sample_drive_row']) h+=textInput(o,k); }
else if(selected.kind=='chrome'){ for(const k of ['x','y','scale_x','scale_y']) h+=numberInput(o,k,k.startsWith('scale')?0.001:0.1); h+=`<label>editable</label><input type="checkbox" ${o.editable?'checked':''} onchange="setBool('editable',this.checked)">`; if(selected.name=='backing') h+=`<div></div><div class="ok">normal non-highlighted row rectangle: live transform via named Backing when connected</div>`; if(selected.name=='cursor') h+=`<div></div><div class="ok">outer highlighted-row wrapper: live transform when connected; use cursor_body for visible highlight-panel height</div>`; if(selected.name=='cursor_body') h+=`<div></div><div class="ok">inner highlighted-row body: live transform via Cursor/CursorBody when connected</div>`; h+=textInput(o,'source'); }
else { for(const k of ['row_pitch','visible_rows','top_recycle_rows','bottom_recycle_rows','mask_height','scrollbar_x','scrollbar_top_y','scrollbar_track_height']) h+=numberInput(o,k,1); }
 h+='</div><div class="controls"><button onclick="nudge(-1,0)">← 1</button><button onclick="nudge(1,0)">→ 1</button><button onclick="nudge(0,-1)">↑ 1</button><button onclick="nudge(0,1)">↓ 1</button><button onclick="nudge(-5,0)">← 5</button><button onclick="nudge(5,0)">→ 5</button><button onclick="nudge(0,-5)">↑ 5</button><button onclick="nudge(0,5)">↓ 5</button></div>'; e.innerHTML=h; }
function setVal(k,v){ getObj()[k]=Number(v); renderEditor(); draw(); }
function setText(k,v){ getObj()[k]=v; renderEditor(); draw(); }
function setBool(k,v){ getObj()[k]=v; renderEditor(); draw(); }
function nudge(dx,dy){ const o=getObj(); if('x' in o) o.x+=dx; if('y' in o) o.y+=dy; renderEditor(); draw(); }
function sampleFor(f){ const m=document.getElementById('mode').value; return f['sample_'+m] || ''; }
function add(tag, attrs, parent=document.getElementById('canvas')){ const el=document.createElementNS('http://www.w3.org/2000/svg',tag); for(const [k,v] of Object.entries(attrs)) el.setAttribute(k,v); parent.appendChild(el); return el; }
function draw(){ const svg=document.getElementById('canvas'); svg.innerHTML=''; const rowPitch=data.list.row_pitch, half=data.list.mask_height/2; for(let y=-half;y<=half;y+=rowPitch) add('line',{x1:-620,x2:640,y1:y,y2:y,class:'rowline'}); add('rect',{x:-620,y:-half,width:1260,height:data.list.mask_height,fill:'none',stroke:'#777','stroke-dasharray':'6 4'}); add('rect',{x:data.list.scrollbar_x,y:data.list.scrollbar_top_y,width:10,height:data.list.scrollbar_track_height,fill:'#8884',stroke:'#aaa'});
 for(const [name,f] of Object.entries(data.fields)){ const sel=selected.kind=='field'&&selected.name==name; const r=add('rect',{x:f.x-2,y:f.y-2,width:f.width,height:40,class:'field',stroke:sel?'#fff':'#6ca0ff','stroke-width':sel?2:1}); bindDrag(r,'field',name); add('text',{x:f.x,y:f.y+f.font_height,class:'label'},svg).textContent=sampleFor(f)||name; add('text',{x:f.x,y:f.y-5,class:'label'},svg).textContent=name; }
 const backing=data.row_chrome.backing; const bx=backing.x-19.1*backing.scale_x, by=backing.y-19.4*backing.scale_y, bw=39*backing.scale_x, bh=37.5*backing.scale_y; let r=add('rect',{x:bx,y:by,width:bw,height:bh,class:'chrome',stroke:selected.kind=='chrome'&&selected.name=='backing'?'#fff':'#ffbd5a','stroke-width':selected.kind=='chrome'&&selected.name=='backing'?2:1}); bindDrag(r,'chrome','backing'); add('text',{x:bx,y:by-4,class:'label'}).textContent='normal backing char54/shape53';
 const c=data.row_chrome.cursor; const cb=data.row_chrome.cursor_body; const cx=c.x+cb.x*c.scale_x, cy=c.y+cb.y*c.scale_y, cw=39*c.scale_x*cb.scale_x, ch=37.5*c.scale_y*cb.scale_y; r=add('rect',{x:cx,y:cy-19.4*c.scale_y*cb.scale_y,width:cw,height:ch,class:'chrome',stroke:selected.kind=='chrome'&&selected.name=='cursor'?'#fff':'#ffbd5a','stroke-width':selected.kind=='chrome'&&selected.name=='cursor'?2:1}); bindDrag(r,'chrome','cursor'); add('text',{x:cx,y:cy-25,class:'label'}).textContent='effective highlight body (cursor + inner body)';
 const body=data.row_chrome.cursor_body; add('rect',{x:body.x,y:body.y-19.4*body.scale_y,width:39*body.scale_x,height:37.5*body.scale_y,class:'readonly'}); add('text',{x:body.x,y:body.y-25,class:'label'}).textContent='raw inner highlight body only'; }
function bindDrag(el,kind,name){ el.onpointerdown=(ev)=>{selected={kind,name};renderTree();renderEditor();drag={kind,name,sx:ev.clientX,sy:ev.clientY,ox:getObj().x,oy:getObj().y}; el.setPointerCapture(ev.pointerId);}; el.onpointermove=(ev)=>{ if(!drag) return; const o=getObj(); o.x=Math.round((drag.ox+(ev.clientX-drag.sx))*10)/10; o.y=Math.round((drag.oy+(ev.clientY-drag.sy))*10)/10; renderEditor(); draw();}; el.onpointerup=()=>drag=null; }
function renderMode(){ return document.getElementById('renderMode').value; }
function renderModeChanged(){ save(); draw(); pollStatus(); }
function selectedNeedsHotGfx(){ return selected.kind!='chrome'; }
async function postSchema(payload){ const r=await fetch('/schema',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)}); const msg=await r.text(); document.getElementById('log').textContent=msg; if(!r.ok) throw new Error(msg); return msg; }
async function save(){ try { const payload={data, render_mode:renderMode(), selected, text_probe:false}; await postSchema(payload); if(selectedNeedsHotGfx()){ await fetch('/rebuild',{method:'POST'}); poll(); } pollStatus(); } catch(e) { showError(e); } }
async function rebuild(){ try { const payload={data, render_mode:renderMode(), selected, text_probe:false}; await postSchema(payload); await fetch('/rebuild',{method:'POST'}); poll(); } catch(e) { showError(e); } }
async function probeText(){ try { const payload={data, render_mode:'live_runtime', selected, text_probe:true}; await postSchema(payload); pollStatus(); } catch(e) { showError(e); } }
async function poll(){ const s=await (await fetch('/rebuild')).json(); document.getElementById('log').textContent=(s.running?'RUNNING\n':'')+(s.output||''); if(s.running) setTimeout(poll,1000); }
async function pollStatus(){ runtimeStatus=await (await fetch('/status')).json(); const mode=renderMode(); const live=mode=='live_runtime'; const ack=runtimeStatus.ack_sequence||0; document.getElementById('runtimeStatus').textContent=JSON.stringify(runtimeStatus,null,2); const badge=document.getElementById('runtimeBadge'); if(!live){ badge.className='warn'; badge.textContent='offline approximate mode: not visually authoritative'; } else if(runtimeStatus.connected){ badge.className='ok'; badge.textContent='live runtime connected: ack '+ack+' status='+runtimeStatus.status; } else { badge.className='bad'; badge.textContent='live runtime requested but disconnected/no ack'; } setTimeout(pollStatus,1000); }
document.addEventListener('keydown',e=>{ if(e.target.tagName=='INPUT')return; const d=e.shiftKey?5:1; if(e.key=='ArrowLeft')nudge(-d,0); if(e.key=='ArrowRight')nudge(d,0); if(e.key=='ArrowUp')nudge(0,-d); if(e.key=='ArrowDown')nudge(0,d); });
Object.assign(window,{load,renderModeChanged,save,rebuild,draw,setVal,setText,setBool,nudge});
document.getElementById('renderMode').addEventListener('change', renderModeChanged);
document.getElementById('saveBtn').addEventListener('click', save);
document.getElementById('rebuildBtn').addEventListener('click', rebuild);
document.getElementById('probeTextBtn').addEventListener('click', probeText);
document.getElementById('reloadBtn').addEventListener('click', load);
document.getElementById('mode').addEventListener('change', draw);
window.addEventListener('error', e=>showError(e.error||e.message));
window.addEventListener('unhandledrejection', e=>showError(e.reason));
load().catch(showError); pollStatus().catch(showError);
</script>
</body>
"""


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def send_bytes(self, status, body: bytes, ctype: str):
        self.send_response(status)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/":
            paths = {"schema": str(SCHEMA), "rebuild": str(REBUILD), "repo": str(REPO), "control": str(CONTROL_FILE), "status": str(STATUS_FILE)}
            body = HTML.replace("%FIELDS%", json.dumps(FIELD_NAMES)).replace("%CHROME%", json.dumps(CHROME_NAMES)).replace("%PATHS%", json.dumps(paths))
            self.send_bytes(200, body.encode(), "text/html; charset=utf-8")
        elif path == "/schema":
            self.send_bytes(200, json.dumps(load_schema()).encode(), "application/json")
        elif path == "/rebuild":
            self.send_bytes(200, json.dumps(rebuild_state).encode(), "application/json")
        elif path == "/status":
            self.send_bytes(200, json.dumps(runtime_status()).encode(), "application/json")
        else:
            self.send_bytes(404, b"not found", "text/plain")

    def do_POST(self):
        path = urlsplit(self.path).path
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        if path == "/schema":
            payload = json.loads(body.decode())
            if "data" in payload:
                data = payload["data"]
                render_mode = payload.get("render_mode", "offline_approximate")
                selected = payload.get("selected", {"kind": "field", "name": "PlayerName"})
                text_probe = bool(payload.get("text_probe", False))
            else:
                data = payload
                render_mode = "offline_approximate"
                selected = {"kind": "field", "name": "PlayerName"}
                text_probe = False
            errors = validate_data(data, selected)
            if errors:
                self.send_bytes(400, ("invalid 05_010 layout:\n" + "\n".join(f"- {e}" for e in errors) + "\n").encode(), "text/plain")
                return
            write_schema(data)
            sequence = write_control(data, render_mode, selected, text_probe)
            self.send_bytes(200, f"saved {SCHEMA}\nwrote {CONTROL_FILE}\nsequence={sequence}\n".encode(), "text/plain")
        elif self.path == "/rebuild":
            start_rebuild()
            self.send_bytes(202, b"rebuild started\n", "text/plain")
        else:
            self.send_bytes(404, b"not found", "text/plain")


def self_test():
    data = load_schema()
    missing = [name for name in FIELD_NAMES if name not in data["fields"]]
    if missing:
        raise SystemExit(f"missing fields: {missing}")
    for name in CHROME_NAMES:
        for key in ["x", "y", "scale_x", "scale_y", "editable", "source"]:
            if key not in data["row_chrome"][name]:
                raise SystemExit(f"missing row_chrome.{name}.{key}")
    errors = validate_data(data)
    if errors:
        raise SystemExit("invalid editor schema:\n" + "\n".join(f"- {e}" for e in errors))
    if not REBUILD.exists():
        raise SystemExit(f"missing rebuild script {REBUILD}")
    seq = write_control(data, "offline_approximate", {"kind": "chrome", "name": "cursor"})
    command = parse_protocol_file(CONTROL_FILE)
    if command is None or int(command.get("sequence", 0)) != seq:
        raise SystemExit("protocol control file write/read failed")
    # Do not delete STATUS_FILE here: an editor self-test may run while Elden Ring is live, and
    # status.txt is the only honest runtime-ack surface the webview has.
    status = runtime_status()
    if "connected" not in status or "ack_sequence" not in status:
        raise SystemExit(f"runtime status parser returned malformed status: {status}")
    print(f"schema={SCHEMA}")
    print(f"control={CONTROL_FILE}")
    print(f"status={STATUS_FILE}")
    print(f"fields={len(data['fields'])} chrome={len(data['row_chrome'])}")
    print("self-test ok")


def main():
    parser = argparse.ArgumentParser(description="05_010_ProfileSelect local visual editor")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8776)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print("05_010 ProfileSelect visual editor")
    print("Approximate canvas only; in-game Scaleform runtime is final visual oracle.")
    print(f"url=http://{args.host}:{args.port}/")
    print(f"schema={SCHEMA}")
    print(f"rebuild={REBUILD}")
    server.serve_forever()


if __name__ == "__main__":
    main()
