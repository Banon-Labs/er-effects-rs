#!/usr/bin/env python3
"""Extract one draw's bound constant buffers + textures from a RenderDoc capture.

Run under qrenderdoc (which bundles the `renderdoc` Python module — the standalone
module is NOT packaged on Arch):

    QT_QPA_PLATFORM=offscreen qrenderdoc --python scripts/extract-capture.py -- \
        <capture.rdc> <out_dir> [--event-id N | --match SUBSTR] [--list]

The capture is a vkd3d-proton -> native Vulkan frame (RenderDoc's Vulkan layer); the
shaders are SPIR-V from the same dxil-spirv we use, so a draw is matched by a substring
of its shader disassembly (e.g. a cbuffer/resource name) or by an explicit RenderDoc
event id. `--list` dumps every drawcall's eid + shader ids so you can pick one in the GUI.

Output (consumed by er_objectkit::capture::Capture):
    <out_dir>/manifest.json
    <out_dir>/cb_<set>_<binding>.bin       raw constant-buffer bytes
    <out_dir>/tex_<set>_<binding>.dds      texture (RenderDoc SaveTexture, DDS; decoded
                                           by image_dds in the replay)

NB: RenderDoc 1.44 replaced BoundResource arrays with a flat UsedDescriptor list. If a
member name below mismatches your build, introspect in the qrenderdoc Python shell
(`help(rd.UsedDescriptor)`, `help(rd.Descriptor)`) and adjust.
"""

import json
import os
import sys

import renderdoc as rd  # provided by qrenderdoc's embedded interpreter


def parse_args(argv):
    # Preferred (robust under qrenderdoc, whose argv passing is unreliable): a JSON config
    # at $EXTRACT_CONFIG or /tmp/er-extract.json with {rdc,out,event_id,match,list}.
    cfg_path = os.environ.get("EXTRACT_CONFIG", "/tmp/er-extract.json")
    if os.path.exists(cfg_path):
        c = json.load(open(cfg_path))
        return (c.get("rdc"), c.get("out"), c.get("event_id"), c.get("match"),
                bool(c.get("list")), bool(c.get("skip_textures")))
    # Fallback: `qrenderdoc --python extract-capture.py -- <rdc> <out> [--event-id N|--match S|--list]`.
    if "--" in argv:
        argv = argv[argv.index("--") + 1 :]
    else:
        argv = [a for a in argv[1:] if not a.endswith("extract-capture.py")]
    rdc = out = None
    event_id = None
    match = None
    do_list = False
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--event-id":
            event_id = int(argv[i + 1]); i += 2
        elif a == "--match":
            match = argv[i + 1]; i += 2
        elif a == "--list":
            do_list = True; i += 1
        elif rdc is None:
            rdc = a; i += 1
        elif out is None:
            out = a; i += 1
        else:
            i += 1
    return rdc, out, event_id, match, do_list, False


def walk(actions):
    for a in actions:
        yield a
        yield from walk(a.children)


def open_capture(path):
    print(f"[t] OpenFile {path} ...")
    cap = rd.OpenCaptureFile()
    res = cap.OpenFile(path, "", None)
    print(f"[t] OpenFile -> {res}")
    if res != rd.ResultCode.Succeeded:
        raise SystemExit(f"OpenFile failed: {res}")
    if not cap.LocalReplaySupport():
        raise SystemExit("no local replay support for this capture")
    print("[t] OpenCapture (init replay; LOADS ALL RESOURCES — minutes on a multi-GB cap) ...")
    res, ctrl = cap.OpenCapture(rd.ReplayOptions(), None)
    print(f"[t] OpenCapture -> {res}")
    if res != rd.ResultCode.Succeeded:
        raise SystemExit(f"OpenCapture failed: {res}")
    return cap, ctrl


def shader_disasm(ctrl, state, stage):
    refl = state.GetShaderReflection(stage)
    if refl is None:
        return ""
    pipe = state.GetGraphicsPipelineObject()
    targets = ctrl.GetDisassemblyTargets(True)
    if not targets:
        return ""
    try:
        return ctrl.DisassembleShader(pipe, refl, targets[0])
    except Exception:
        return ""


def run_extract(rdc, out, event_id, match, do_list, skip_textures):
    cap, ctrl = open_capture(rdc)
    draws = [a for a in walk(ctrl.GetRootActions()) if a.flags & rd.ActionFlags.Drawcall]
    sdf = ctrl.GetStructuredFile()

    if do_list:
        # Lightweight: action tree only (no per-draw replay) so it's fast on a multi-GB cap.
        for a in draws:
            print(f"eid={a.eventId:>6}  numIndices={a.numIndices:>7}  {a.GetName(sdf)}")
        ctrl.Shutdown(); cap.Shutdown(); os._exit(0)

    # Select the target draw.
    target = None
    if event_id is not None:
        target = next((a for a in draws if a.eventId == event_id), None)
    elif match is not None:
        for a in draws:
            ctrl.SetFrameEvent(a.eventId, True)
            st = ctrl.GetPipelineState()
            if match in shader_disasm(ctrl, st, rd.ShaderStage.Pixel) or match in shader_disasm(
                ctrl, st, rd.ShaderStage.Vertex
            ):
                target = a
                break
    if target is None:
        print("ERROR: no matching draw; re-run with --list to pick an --event-id")
        os._exit(2)

    os.makedirs(out, exist_ok=True)
    print(f"[t] SetFrameEvent(eid={target.eventId}) — replaying to this draw ...")
    ctrl.SetFrameEvent(target.eventId, True)
    print(f"[t] at eid {target.eventId}; reading pipeline state + cbuffers ...")
    st = ctrl.GetPipelineState()
    manifest = {"draw": target.GetName(sdf), "buffers": [], "textures": []}

    def d3d_register(refl_entry, fallback):
        # The D3D shader register (bN/tN). Member name shifted across RenderDoc versions;
        # `fixedBindNumber` is current, `bindPoint` older. Falls back to the access index.
        for attr in ("fixedBindNumber", "bindPoint", "bind"):
            v = getattr(refl_entry, attr, None)
            if v is not None:
                return int(v)
        return int(fallback)

    for stage, sc in (("vertex", rd.ShaderStage.Vertex), ("pixel", rd.ShaderStage.Pixel)):
        refl = st.GetShaderReflection(sc)
        cbs = st.GetConstantBlocks(sc, True) if refl is not None else []
        ro_used = st.GetReadOnlyResources(sc, True) if refl is not None else []
        print(f"  [stage {stage}] reflection={'present' if refl else 'NONE'} "
              f"cbuffers={len(cbs)} textures={len(ro_used)}")
        if refl is None:
            continue
        cblocks = list(refl.constantBlocks)
        ro = list(refl.readOnlyResources)
        # Constant buffers bound to this stage. NB: vkd3d-proton's descriptor buffers make
        # every register report as 0, so the FILENAME must use a running index, not the
        # register — else all cbuffers overwrite one file. The register is kept (informational)
        # but the replay re-associates by byte SIZE, not register.
        for slot, ud in enumerate(cbs):
            d = ud.descriptor
            rid = d.resource
            if rid == rd.ResourceId.Null():
                continue
            off = getattr(d, "byteOffset", 0)
            sz = getattr(d, "byteSize", 0)
            data = bytes(ctrl.GetBufferData(rid, off, sz))
            idx = ud.access.index
            entry = cblocks[idx] if 0 <= idx < len(cblocks) else None
            reg = d3d_register(entry, idx) if entry is not None else idx
            name = str(entry.name) if entry is not None else f"cb{idx}"
            fn = f"cb_{stage}_{slot}.bin"
            print(f"  [cb] {stage} slot{slot} b{reg} {name} {len(data)}B")
            with open(os.path.join(out, fn), "wb") as f:
                f.write(data)
            manifest["buffers"].append(
                {"register": reg, "name": name, "stage": stage, "kind": "uniform",
                 "file": fn, "size": len(data)}
            )
        # Read-only textures (SRVs) bound to this stage — DDS via SaveTexture.
        for ud in [] if skip_textures else st.GetReadOnlyResources(sc, True):
            d = ud.descriptor
            rid = d.resource
            if rid == rd.ResourceId.Null():
                continue
            idx = ud.access.index
            entry = ro[idx] if 0 <= idx < len(ro) else None
            reg = d3d_register(entry, idx) if entry is not None else idx
            name = str(entry.name) if entry is not None else f"t{idx}"
            fn = f"tex_{stage}_{reg}.dds"
            ts = rd.TextureSave()
            ts.resourceId = rid
            ts.mip = -1               # all mips
            ts.slice.sliceIndex = -1  # all slices/faces
            ts.destType = rd.FileType.DDS
            try:
                ctrl.SaveTexture(ts, os.path.join(out, fn))
                manifest["textures"].append(
                    {"register": reg, "name": name, "stage": stage, "file": fn}
                )
            except Exception as e:
                print(f"  WARN texture reg t{reg} ({name}): {e}")

    with open(os.path.join(out, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
    print(f"wrote {out}/manifest.json: {len(manifest['buffers'])} buffers, "
          f"{len(manifest['textures'])} textures (draw eid={target.eventId})")

    ctrl.Shutdown()
    cap.Shutdown()
    os._exit(0)


def _thread_excepthook(args):
    import traceback
    print("EXTRACT ERROR:\n" + "".join(
        traceback.format_exception(args.exc_type, args.exc_value, args.exc_traceback)))
    os._exit(3)


def main():
    # qrenderdoc swallows print()/stdout into its own console, so log to a file the host
    # shell can read. Run the extraction on a WORKER thread and return, so qrenderdoc's main
    # thread is free to pump the Qt event loop that the replay/readback calls depend on —
    # calling SetFrameEvent/GetBufferData on the main thread deadlocks.
    import threading
    logpath = os.environ.get("EXTRACT_LOG", "/tmp/er-extract.log")
    sys.stdout = sys.stderr = open(logpath, "w", buffering=1)
    rdc, out, event_id, match, do_list, skip_textures = parse_args(sys.argv)
    print(f"extract-capture: rdc={rdc} out={out} event_id={event_id} match={match} "
          f"list={do_list} skip_textures={skip_textures}")
    if not rdc or (not out and not do_list):
        print("ERROR: need rdc + (out or list); see /tmp/er-extract.json")
        os._exit(1)
    threading.excepthook = _thread_excepthook
    print("[t] extraction on worker thread; main() returns so qrenderdoc pumps its event loop")
    threading.Thread(
        target=run_extract,
        args=(rdc, out, event_id, match, do_list, skip_textures),
        daemon=False,
    ).start()


main()
