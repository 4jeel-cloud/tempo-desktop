#!/usr/bin/env python3

"""Generate every app, tray and packaging icon from the master logo.

The master is `assets/icon.png`: a square, transparent-background raster of the app mark
with no border. Everything here is derived from it, so the logo is changed in one place.
The mark is drawn edge to edge inside its own canvas; the platform shapes differ only in
the mask applied to the transparent margin, never in a painted background, so no icon ever
carries a white or black border.
"""

import struct
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
MASTER = ROOT / "assets" / "icon.png"

LINUX_SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
WINDOWS_SIZES = [16, 24, 32, 48, 64, 128, 256]
TRAY_SIZES = [32, 64]

ICNS_TYPES = [
    ("icp4", 16),
    ("icp5", 32),
    ("ic11", 32),
    ("ic12", 64),
    ("ic07", 128),
    ("ic13", 256),
    ("ic08", 256),
    ("ic14", 512),
    ("ic09", 512),
    ("ic10", 1024),
]

# How much of a 1024 canvas the macOS squircle occupies, matching Apple's grid.
SQUIRCLE_FILL = 824 / 1024
SQUIRCLE_RADIUS = 185.4 / 824
# How much of a 1024 canvas the mark itself occupies on macOS, so the squircle has margin.
MACOS_MARK = 0.72
# The corner radius of the Windows tile, as a fraction of the side.
WINDOWS_RADIUS = 0.1875
# How much of the canvas the Linux circle and Windows tile leave as transparent margin.
ROUND_FILL = 0.86


def scaled(master, side):
    return master.resize((side, side), Image.LANCZOS)


def contained(master, canvas, fill):
    """The master centred in a square `canvas` at `fill` of its width, on transparency."""
    side = max(1, round(canvas * fill))
    mark = scaled(master, side)
    out = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    offset = (canvas - side) // 2
    out.alpha_composite(mark, (offset, offset))
    return out


def in_circle(master, canvas, fill):
    """The mark inside a transparent circle, its glyph cropped to the circle."""
    side = max(1, round(canvas * fill))
    mark = scaled(master, side)
    mask = Image.new("L", (side, side), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, side - 1, side - 1), fill=255)
    mark.putalpha(ImageChops.multiply(mark.getchannel("A"), mask))
    out = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    offset = (canvas - side) // 2
    out.alpha_composite(mark, (offset, offset))
    return out


def in_rounded(master, canvas, fill, radius):
    """The mark inside a transparent rounded square, its glyph cropped to the tile."""
    side = max(1, round(canvas * fill))
    mark = scaled(master, side)
    mask = Image.new("L", (side, side), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, side - 1, side - 1), radius=round(side * radius), fill=255
    )
    mark.putalpha(ImageChops.multiply(mark.getchannel("A"), mask))
    out = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    offset = (canvas - side) // 2
    out.alpha_composite(mark, (offset, offset))
    return out


def macos_tile(master, side):
    """The macOS icon: the mark inside Apple's squircle, on transparency between them."""
    tile = in_rounded(master, side, SQUIRCLE_FILL, SQUIRCLE_RADIUS)
    # The mark sits well inside the squircle, so the tile reads as an icon with padding.
    glyph = contained(master, side, MACOS_MARK)
    return Image.alpha_composite(tile, glyph)


def white_silhouette(master, side):
    """The tray glyph: the mark's shape in white, with the master's own alpha as the shape."""
    mark = scaled(master, side)
    silhouette = Image.new("RGBA", (side, side), (255, 255, 255, 0))
    silhouette.putalpha(mark.getchannel("A"))
    return silhouette


def png_bytes(image):
    import io

    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    return buffer.getvalue()


def write_ico(entries, path):
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = len(header) + 16 * len(entries)
    directory = b""
    payload = b""
    for pixels, blob in entries:
        directory += struct.pack(
            "<BBBBHHII",
            0 if pixels >= 256 else pixels,
            0 if pixels >= 256 else pixels,
            0,
            0,
            1,
            32,
            len(blob),
            offset,
        )
        payload += blob
        offset += len(blob)
    path.write_bytes(header + directory + payload)


def write_icns(entries, path):
    body = b""
    for kind, blob in entries:
        body += kind.encode("ascii") + struct.pack(">I", len(blob) + 8) + blob
    path.write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


def write_readme_svg(master):
    """The README header mark, embedded as a base64 PNG so it reads as the logo does."""
    import base64

    side = 128
    blob = png_bytes(contained(master, side, 1.0))
    encoded = base64.b64encode(blob).decode("ascii")
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="60" height="60"'
        f' viewBox="0 0 {side} {side}">\n'
        f'  <image width="{side}" height="{side}"'
        f' href="data:image/png;base64,{encoded}"/>\n'
        f"</svg>\n"
    )


def main():
    if not MASTER.exists():
        sys.exit(f"icons: {MASTER} is missing")

    master = Image.open(MASTER).convert("RGBA")
    if master.width != master.height:
        sys.exit("icons: master is not square")
    if master.getbbox() is None:
        sys.exit("icons: master is fully transparent")

    assets = ROOT / "assets"

    assets.joinpath("linux", "sonora.svg").write_text(
        write_readme_svg(master), encoding="utf-8"
    )
    (ROOT / ".github" / "readme-icon.svg").write_text(
        write_readme_svg(master), encoding="utf-8"
    )

    for pixels in LINUX_SIZES:
        target = (
            assets
            / "linux"
            / "icons"
            / "hicolor"
            / f"{pixels}x{pixels}"
            / "apps"
            / "sonora.png"
        )
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(png_bytes(in_circle(master, pixels, ROUND_FILL)))

    icns = []
    rendered = {}
    for kind, pixels in ICNS_TYPES:
        if pixels not in rendered:
            rendered[pixels] = png_bytes(macos_tile(master, pixels))
        icns.append((kind, rendered[pixels]))
    write_icns(icns, assets / "macos" / "sonora.icns")

    ico = [
        (pixels, png_bytes(in_rounded(master, pixels, ROUND_FILL, WINDOWS_RADIUS)))
        for pixels in WINDOWS_SIZES
    ]
    windows = assets / "windows"
    windows.mkdir(parents=True, exist_ok=True)
    write_ico(ico, windows / "sonora.ico")

    tray = assets / "tray"
    tray.mkdir(parents=True, exist_ok=True)
    for pixels in TRAY_SIZES:
        (tray / f"template-{pixels}.png").write_bytes(
            png_bytes(white_silhouette(master, pixels))
        )
    (tray / "sonora.png").write_bytes(png_bytes(in_circle(master, 32, ROUND_FILL)))

    print(
        f"icons: {len(LINUX_SIZES)} linux png, 1 linux svg, 1 icns, 1 ico,"
        f" {len(TRAY_SIZES) + 1} tray png, 1 readme svg"
    )


if __name__ == "__main__":
    main()
