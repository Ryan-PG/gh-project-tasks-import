#!/usr/bin/env python3
"""Generate the source app icon as a 1024x1024 RGBA PNG.

Pure stdlib (zlib + struct) so it runs anywhere without Pillow. The mark is a
rounded-square gradient tile with a white "import" arrow dropping into a tray.

Run from the `desktop/` directory:

    python scripts/make_icon.py

then feed the result to the Tauri CLI, which emits every size the bundler wants:

    npx tauri icon src-tauri/icons/source.png
"""

import math
import struct
import zlib
from pathlib import Path

SIZE = 1024
OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "icons" / "source.png"

# Tile gradient, top to bottom.
TOP = (0x1F, 0x29, 0x37)
BOTTOM = (0x4F, 0x46, 0xE5)

CORNER_RADIUS = 0.22  # as a fraction of the tile size


def smoothstep(edge0: float, edge1: float, x: float) -> float:
    t = max(0.0, min(1.0, (x - edge0) / (edge1 - edge0)))
    return t * t * (3.0 - 2.0 * t)


def rounded_rect_sdf(px, py, cx, cy, hw, hh, r):
    """Signed distance to a rounded rectangle centred at (cx, cy)."""
    qx = abs(px - cx) - (hw - r)
    qy = abs(py - cy) - (hh - r)
    ax, ay = max(qx, 0.0), max(qy, 0.0)
    return math.hypot(ax, ay) + min(max(qx, qy), 0.0) - r


def triangle_sdf(px, py, ax, ay, bx, by, cx, cy):
    """Signed distance to a triangle, via the three half-plane edges."""
    def edge(x0, y0, x1, y1):
        # Positive on the inside for the winding order we use.
        return (x1 - x0) * (py - y0) - (y1 - y0) * (px - x0)

    e0 = edge(ax, ay, bx, by)
    e1 = edge(bx, by, cx, cy)
    e2 = edge(cx, cy, ax, ay)
    d = max(e0, e1, e2)
    # Normalise by the longest edge so the value is roughly a distance.
    scale = max(
        math.hypot(bx - ax, by - ay),
        math.hypot(cx - bx, cy - by),
        math.hypot(ax - cx, ay - cy),
    ) or 1.0
    return -d / scale


def coverage(sdf: float, aa: float) -> float:
    """Antialias a signed distance into 0..1 coverage."""
    return 1.0 - smoothstep(-aa, aa, sdf)


def render() -> bytes:
    n = SIZE
    aa = 1.0 / n  # one pixel of feathering in normalised units

    # Glyph geometry, in normalised 0..1 coordinates.
    shaft = (0.5, 0.36, 0.040, 0.115)          # cx, cy, half-width, half-height
    head = (0.5, 0.60, 0.355, 0.445, 0.635, 0.445)  # apex, left, right
    tray = (0.5, 0.735, 0.235, 0.042)          # cx, cy, half-width, half-height

    rows = bytearray()
    for y in range(n):
        rows.append(0)  # PNG filter type 0
        py = (y + 0.5) / n
        for x in range(n):
            px = (x + 0.5) / n

            # Tile: rounded square, gradient fill.
            tile = coverage(
                rounded_rect_sdf(px, py, 0.5, 0.5, 0.5, 0.5, CORNER_RADIUS), aa
            )
            if tile <= 0.0:
                rows.extend((0, 0, 0, 0))
                continue

            t = py
            r = round(TOP[0] + (BOTTOM[0] - TOP[0]) * t)
            g = round(TOP[1] + (BOTTOM[1] - TOP[1]) * t)
            b = round(TOP[2] + (BOTTOM[2] - TOP[2]) * t)

            # Glyph, composited over the tile in white.
            s = min(
                rounded_rect_sdf(px, py, shaft[0], shaft[1], shaft[2], shaft[3], shaft[2]),
                triangle_sdf(px, py, head[0], head[1], head[2], head[3], head[4], head[5]),
                rounded_rect_sdf(px, py, tray[0], tray[1], tray[2], tray[3], tray[3]),
            )
            w = coverage(s, aa)
            if w > 0.0:
                r = round(r + (255 - r) * w)
                g = round(g + (255 - g) * w)
                b = round(b + (255 - b) * w)

            rows.extend((r, g, b, round(255 * tile)))

    return bytes(rows)


def chunk(tag: bytes, data: bytes) -> bytes:
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def main() -> None:
    raw = render()
    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)  # 8-bit RGBA
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(png)
    print(f"wrote {OUT} ({len(png):,} bytes, {SIZE}x{SIZE})")


if __name__ == "__main__":
    main()
