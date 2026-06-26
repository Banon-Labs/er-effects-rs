# RenderDoc frame extraction (M3 bindings / M4 cbuffers)

Turns a RenderDoc `.rdc` into per-draw JSON: bound shaders, **constant-buffer
contents** (`cbInstanceData` world/view/proj/light, `cbMtdParam`), and the
texture->register mapping. That mapping disambiguates the ~23 textures / ~14 buffers
an ER pixel shader binds (only ~10 textures are the material's; the rest are engine
IBL/shadow/scene resources), and the cbuffer contents are the exact engine values the
offline render otherwise has to synthesize.

The Arch `renderdoc` package ships only `librenderdoc.so` -- **no headless python
module** -- so resolved cbuffer *contents* need either `qrenderdoc`'s embedded
interpreter (`extract_frame.py`) OR parsing the headless XML below.

## The Wayland blocker (must read)

RenderDoc does **not** support capturing a client on Wayland directly (qrenderdoc
logs *"Running directly on Wayland is NOT SUPPORTED"* and captures nothing). On
Hyprland, force the captured app onto **XWayland** by unsetting `WAYLAND_DISPLAY`
(winit/Bevy then uses X11 on `DISPLAY=:0`); run `qrenderdoc` itself with
`QT_QPA_PLATFORM=xcb` if you use the GUI.

## Step 0 -- PROVEN headless capture of our own viewer (no GUI)

The viewer self-captures via the in-app RenderDoc API (`--rdc-capture`). Run
successfully 2026-06-25; produced a 223 MB `.rdc` with all draws/descriptors:

```
cargo build -p er-shader-viewer
env -u WAYLAND_DISPLAY DISPLAY=:0 \
  renderdoccmd capture -w -c /tmp/er-cap \
  target/debug/er-shader-viewer --object c4800 --rdc-capture /tmp/er-cap
# -> /tmp/er-cap_frameNN.rdc

renderdoccmd convert -f /tmp/er-cap_frameNN.rdc -i rdc -c zip.xml -o /tmp/er-cap.zip.xml
# -> headless Vulkan chunk XML: vkCmdBindDescriptorSets / vkUpdateDescriptorSets /
#    vkCmdDrawIndexed / vkCreateShaderModule ... (the bind structure to parse)
```

For resolved cbuffer *contents*, instead open the `.rdc` in
`QT_QPA_PLATFORM=xcb qrenderdoc`, `Window > Python Shell`, and run
`scripts/renderdoc/extract_frame.py` -> `/tmp/er-frame.json`.

## Step 1 -- the gated ER capture (the real goal, M4)

Same flow, but the target is the **approved offline, EAC-free `eldenring.exe`** probe
path under Proton, with RenderDoc's Vulkan capture layer injected into the Proton
prefix. Whether RenderDoc attaches through vkd3d-proton + Arxan is the open runtime
unknown -- that's the experiment to run. If it attaches, capture a frame with the
object on screen and run the same script. If it doesn't, we fall back to the
in-process memory-read cbuffer oracle.

> Do not launch the EAC/Steam build under RenderDoc -- only the gated offline path.
