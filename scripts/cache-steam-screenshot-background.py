#!/usr/bin/env python3
"""Populate the local boot-background cache from the latest Steam screenshot.

The game DLL reads only the local ERBGRA01 cache written by this helper; it never
scrapes Steam or downloads anything on the launch path.
"""

from __future__ import annotations

import argparse
import html
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

MAGIC = b"ERBGRA01"
APPID_ELDEN_RING = "1245620"
DEFAULT_GAME_DIR_CANDIDATES = [
    Path.home() / ".steam/steam/steamapps/common/ELDEN RING/Game",
    Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game",
    Path("/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"),
]
DEFAULT_CACHE_NAME = "er-effects-boot-background.rgba"
DEFAULT_MAX_BYTES = 2_000_000
DEFAULT_MAX_DIM = 1280


def ensure_pillow() -> None:
    try:
        import PIL.Image  # noqa: F401
        import PIL.ImageOps  # noqa: F401
        return
    except ImportError:
        pass
    if os.environ.get("ER_EFFECTS_CACHE_BG_NO_UV") == "1":
        raise SystemExit("Pillow is required; install it or run via `uv run --with pillow ...`")
    uv = shutil_which("uv")
    if not uv:
        raise SystemExit("Pillow is required and `uv` was not found for ephemeral provisioning")
    env = os.environ.copy()
    env["ER_EFFECTS_CACHE_BG_NO_UV"] = "1"
    os.execvpe(uv, [uv, "run", "--with", "pillow", "python3", *sys.argv], env)


def shutil_which(name: str) -> str | None:
    for part in os.environ.get("PATH", "").split(os.pathsep):
        p = Path(part) / name
        if p.exists() and os.access(p, os.X_OK):
            return str(p)
    return None


def steam_userdata_roots(extra: list[Path]) -> list[Path]:
    roots = [Path.home() / ".local/share/Steam/userdata", Path.home() / ".steam/steam/userdata"]
    roots.extend(extra)
    out: list[Path] = []
    seen: set[Path] = set()
    for root in roots:
        try:
            key = root.resolve()
        except OSError:
            key = root
        if key not in seen and root.exists():
            seen.add(key)
            out.append(root)
    return out


def latest_local_screenshot(appid: str, roots: list[Path]) -> Path | None:
    candidates: list[tuple[float, Path]] = []
    for root in roots:
        for path in root.glob(f"*/760/remote/{appid}/screenshots/*"):
            if path.is_file() and path.suffix.lower() in {".jpg", ".jpeg", ".png"}:
                try:
                    candidates.append((path.stat().st_mtime, path))
                except OSError:
                    pass
    if not candidates:
        return None
    return max(candidates, key=lambda item: item[0])[1]


def account_ids(roots: list[Path]) -> list[int]:
    ids: list[int] = []
    for root in roots:
        for child in root.iterdir() if root.exists() else []:
            if child.is_dir() and child.name.isdigit():
                ids.append(int(child.name))
    return sorted(set(ids))


def request(url: str, timeout: float, *, method: str = "GET", max_bytes: int | None = None) -> bytes:
    req = urllib.request.Request(url, method=method, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        if method == "HEAD":
            return b""
        data = resp.read(max_bytes + 1 if max_bytes is not None else -1)
        if max_bytes is not None and len(data) > max_bytes:
            raise RuntimeError(f"download exceeded {max_bytes} bytes")
        return data


def latest_remote_screenshot_url(appid: str, steamid64: int, timeout: float, max_bytes: int) -> str | None:
    page = (
        f"https://steamcommunity.com/profiles/{steamid64}/screenshots/"
        f"?appid={urllib.parse.quote(appid)}&sort=newestfirst&browsefilter=myfiles&view=imagewall"
    )
    text = request(page, timeout, max_bytes=max_bytes).decode("utf-8", "replace")
    ids: list[str] = []
    for match in re.finditer(r"sharedfiles/filedetails/\?id=(\d+)", text):
        fid = match.group(1)
        if fid not in ids:
            ids.append(fid)
    for fid in ids:
        detail = request(
            f"https://steamcommunity.com/sharedfiles/filedetails/?id={fid}",
            timeout,
            max_bytes=max_bytes,
        ).decode("utf-8", "replace")
        marker_count = len(re.findall(r"app/1245620|ELDEN RING|Elden Ring", detail, re.I))
        media = re.search(r'id="ActualMedia"[^>]*src="([^"]+)"', detail)
        if marker_count and media:
            return html.unescape(media.group(1))
    return None


def download_remote_image(url: str, timeout: float, max_bytes: int) -> bytes:
    # The Steam media URL accepts range requests; HEAD commonly returns Content-Length for a cheap cap.
    try:
        req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            length = resp.headers.get("Content-Length")
            if length and int(length) > max_bytes:
                raise RuntimeError(f"remote image is {length} bytes, cap is {max_bytes}")
    except urllib.error.HTTPError:
        raise
    except Exception:
        # Some CDNs dislike HEAD; the bounded GET below remains authoritative.
        pass
    return request(url, timeout, max_bytes=max_bytes)


def decode_rgba(source: Path | bytes, max_dim: int) -> tuple[int, int, bytes]:
    from io import BytesIO

    from PIL import Image, ImageOps

    if isinstance(source, Path):
        img = Image.open(source)
    else:
        img = Image.open(BytesIO(source))
    with img:
        img = ImageOps.exif_transpose(img).convert("RGBA")
        if max(img.size) > max_dim:
            img.thumbnail((max_dim, max_dim), Image.Resampling.LANCZOS)
        width, height = img.size
        return width, height, img.tobytes()


def write_cache(path: Path, width: int, height: int, rgba: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = MAGIC + struct.pack("<II", width, height) + rgba
    fd, tmp = tempfile.mkstemp(prefix=path.name + ".", suffix=".tmp", dir=str(path.parent))
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(payload)
        os.replace(tmp, path)
    finally:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--appid", default=APPID_ELDEN_RING)
    default_game_dir = next((p for p in DEFAULT_GAME_DIR_CANDIDATES if p.exists()), Path.cwd())
    parser.add_argument("--game-dir", type=Path, default=default_game_dir)
    parser.add_argument("--output", type=Path, help=f"default: <game-dir>/{DEFAULT_CACHE_NAME}")
    parser.add_argument("--steam-userdata", type=Path, action="append", default=[])
    parser.add_argument("--allow-remote", action="store_true", help="scrape public Steam Community pages if local cache misses")
    parser.add_argument("--steamid64", type=int, help="SteamID64 for remote scrape; default derives from local userdata account id")
    parser.add_argument("--timeout", type=float, default=4.0, help="per-request network timeout in seconds")
    parser.add_argument("--max-download-bytes", type=int, default=DEFAULT_MAX_BYTES)
    parser.add_argument("--max-dim", type=int, default=DEFAULT_MAX_DIM, help="max cached width/height after decode")
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    ensure_pillow()
    roots = steam_userdata_roots(args.steam_userdata)
    output = args.output or (args.game_dir / DEFAULT_CACHE_NAME)

    source_desc: str
    image_source: Path | bytes | None = None
    local = latest_local_screenshot(args.appid, roots)
    if local:
        source_desc = f"local:{local}"
        image_source = local
    elif args.allow_remote:
        steamid64 = args.steamid64
        if steamid64 is None:
            raise SystemExit("remote scrape requires --steamid64; local userdata account IDs are not SteamID64 values")
        url = latest_remote_screenshot_url(args.appid, steamid64, args.timeout, args.max_download_bytes)
        if not url:
            raise SystemExit("no public remote screenshot media found")
        started = time.monotonic()
        data = download_remote_image(url, args.timeout, args.max_download_bytes)
        elapsed_ms = int((time.monotonic() - started) * 1000)
        source_desc = f"remote:{len(data)}B:{elapsed_ms}ms:{url[:96]}"
        image_source = data
    else:
        raise SystemExit("no local screenshot found; rerun with --allow-remote to use public Steam Community screenshots")

    width, height, rgba = decode_rgba(image_source, args.max_dim)
    if args.dry_run:
        print(f"would write {output} from {source_desc} decoded={width}x{height} raw={len(rgba)}B")
        return 0
    write_cache(output, width, height, rgba)
    print(f"wrote {output} from {source_desc} decoded={width}x{height} raw={len(rgba)}B")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
