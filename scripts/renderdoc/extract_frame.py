"""Dump a RenderDoc frame capture to JSON: per draw call, the bound shaders, their
constant-buffer CONTENTS, and read-only texture bindings.

The Arch `renderdoc` package ships only librenderdoc.so (no headless python module),
so run this inside qrenderdoc's embedded interpreter:

    1. Open the .rdc:   qrenderdoc your_capture.rdc
    2. Window > Python Shell
    3. (optional) set the output path:  import os; os.environ['ER_FRAME_OUT']='/tmp/er-frame.json'
    4. Run this file:   Open... (folder icon in the shell) -> this script, or paste it.

Output (default /tmp/er-frame.json) is consumed by er-objectkit to drive an
exact in-game-matched passthrough render (cbInstanceData world/view/proj/light,
cbMtdParam, and the real texture->register mapping that disambiguates the
~23 textures / ~14 buffers the ER pixel shaders bind).

Best-effort against RenderDoc 1.44 — if an API call differs on your version it is
caught per-draw and recorded as an "error" field; report those and they're easy to fix.
"""

import json
import os

import renderdoc as rd  # provided by qrenderdoc's interpreter

OUT = os.environ.get("ER_FRAME_OUT", "/tmp/er-frame.json")
# Only dump draws with at least this many indices (skip fullscreen/UI/clears).
MIN_INDICES = int(os.environ.get("ER_FRAME_MIN_INDICES", "300"))


def var_to_obj(v):
    o = {"name": v.name, "rows": v.rows, "cols": v.columns}
    if len(v.members):
        o["members"] = [var_to_obj(m) for m in v.members]
    else:
        n = max(1, v.rows) * max(1, v.columns)
        for arr, key in (("f32v", "f32"), ("u32v", "u32"), ("s32v", "s32")):
            try:
                seq = getattr(v.value, arr)
                o[key] = [seq[i] for i in range(n)]
                break
            except Exception:
                continue
    return o


def stage_dump(ctrl, pipe, stage):
    refl = pipe.GetShaderReflection(stage)
    if refl is None:
        return None
    sd = {"entry": refl.entryPoint, "cbuffers": [], "textures": []}
    pipeobj = pipe.GetGraphicsPipelineObject()
    shader = pipe.GetShader(stage)

    for i, cb in enumerate(refl.constantBlocks):
        try:
            bound = pipe.GetConstantBuffer(stage, i, 0)
            variables = ctrl.GetCBufferVariableContents(
                pipeobj, shader, stage, refl.entryPoint, i,
                bound.resourceId, bound.byteOffset, bound.byteSize,
            )
            sd["cbuffers"].append(
                {"name": cb.name, "vars": [var_to_obj(v) for v in variables]}
            )
        except Exception as e:  # noqa: BLE001
            sd["cbuffers"].append({"name": cb.name, "error": repr(e)})

    try:
        for ro in pipe.GetReadOnlyResources(stage):
            descs = getattr(ro, "resources", None) or getattr(ro, "descriptor", None)
            bind = getattr(ro, "bindPoint", None) or getattr(ro, "firstIndex", None)
            seq = descs if isinstance(descs, (list, tuple)) else [descs]
            for d in seq:
                rid = getattr(d, "resourceId", None)
                if rid is not None and rid != rd.ResourceId.Null():
                    sd["textures"].append({"bind": str(bind), "resource": str(rid)})
    except Exception as e:  # noqa: BLE001
        sd["textures_error"] = repr(e)
    return sd


def extract(ctrl):
    res_names = {}
    try:
        res_names = {r.resourceId: r.name for r in ctrl.GetResources()}
    except Exception:
        pass
    sdfile = ctrl.GetStructuredFile()
    draws = []

    def walk(actions):
        for a in actions:
            if a.flags & rd.ActionFlags.Drawcall and a.numIndices >= MIN_INDICES:
                try:
                    ctrl.SetFrameEvent(a.eventId, True)
                    pipe = ctrl.GetPipelineState()
                    entry = {
                        "eventId": int(a.eventId),
                        "name": a.GetName(sdfile),
                        "numIndices": int(a.numIndices),
                        "numInstances": int(a.numInstances),
                        "vs": stage_dump(ctrl, pipe, rd.ShaderStage.Vertex),
                        "ps": stage_dump(ctrl, pipe, rd.ShaderStage.Pixel),
                    }
                    # resolve texture resourceIds to names
                    for st in ("vs", "ps"):
                        if entry[st]:
                            for t in entry[st]["textures"]:
                                rid_str = t["resource"]
                                t["name"] = next(
                                    (n for r, n in res_names.items() if str(r) == rid_str),
                                    rid_str,
                                )
                    draws.append(entry)
                except Exception as e:  # noqa: BLE001
                    draws.append({"eventId": int(a.eventId), "error": repr(e)})
            walk(a.children)

    walk(ctrl.GetRootActions())
    return draws


_result = {}
pyrenderdoc.Replay().BlockInvoke(lambda c: _result.update(draws=extract(c)))  # noqa: F821

with open(OUT, "w") as f:
    json.dump(_result, f, indent=2, default=str)
print("wrote", len(_result.get("draws", [])), "draws to", OUT)
