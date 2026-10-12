#!/usr/bin/env python3
"""Write the source inputs of the icon and cursor fixtures, bit for bit reproducible.

These files are committed. They are deliberately not in the canonical form the crate writes,
so the tests exercise decoding, validation and re-encoding:

tests/fixtures/icon/source/
  legacy.ico      16x16 bitmap with 4 bits, a 16-color table and a transparent black corner
                  (directory: planes 0, 0 bits),
                  32x32 bitmap with 32 bits, 256x256 RGBA PNG stored with filter None
  layer_16.png    RGBA layers, one file per size
  layer_32.png
  layer_256.png
tests/fixtures/cursor/source/
  legacy.cur      32x32 bitmap with hotspot (5, 7), 256x256 PNG with hotspot (40, 56)

The canonical fixtures next to them (app.ico, second.ico, arrow.cur) are what the crate writes
with to_ico and to_cur after loading these files.
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent.parent / "tests" / "fixtures"
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def padded_row(bits: int) -> int:
    """Bytes of a bitmap row with the given number of bits, padded to a DWORD."""
    return (bits + 31) // 32 * 4


def bitmap(size: int, bit_count: int, palette: list[tuple[int, int, int]], pixel, transparent) -> bytes:
    """BITMAPINFOHEADER, color table, XOR image and AND mask; rows are stored bottom-up."""
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, bit_count, 0, 0, 0, 0, 0, 0)
    table = b"".join(struct.pack("<BBBB", blue, green, red, 0) for red, green, blue in palette)

    xor_rows = []
    and_rows = []
    for y in reversed(range(size)):
        if bit_count == 32:
            row = b"".join(struct.pack("<BBBB", *pixel(x, y)) for x in range(size))
        else:
            per_byte = 8 // bit_count
            row = bytearray()
            for x in range(0, size, per_byte):
                value = 0
                for sub in range(per_byte):
                    value = (value << bit_count) | pixel(x + sub, y)
                row.append(value)
            row = bytes(row)
        xor_rows.append(row.ljust(padded_row(size * bit_count), b"\0"))

        mask = bytearray(padded_row(size))
        for x in range(size):
            if transparent(x, y):
                mask[x // 8] |= 0x80 >> (x % 8)
        and_rows.append(bytes(mask))
    return header + table + b"".join(xor_rows) + b"".join(and_rows)


def png_rgba(size: int, pixel) -> bytes:
    """PNG with color type 6 and 8 bits per channel; every row uses filter None."""
    raw = bytearray()
    for y in range(size):
        raw.append(0)
        for x in range(size):
            raw += bytes(pixel(x, y))

    def chunk(kind: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(kind + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return PNG_SIGNATURE + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + chunk(b"IEND", b"")


def container(file_type: int, images: list[tuple[int, int, int, int, bytes]]) -> bytes:
    """ICONDIR, one ICONDIRENTRY per (size, colors, planes or x, bits or y, data), then the data."""
    offset = 6 + 16 * len(images)
    header = struct.pack("<HHH", 0, file_type, len(images))
    entries = bytearray()
    blobs = bytearray()
    for size, colors, field_a, field_b, data in images:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, colors, 0, field_a, field_b, len(data), offset)
        blobs += data
        offset += len(data)
    return header + bytes(entries) + bytes(blobs)


def disc(size: int):
    """RGBA pixel function: colored gradient inside a disc, transparent outside."""
    center = (size - 1) / 2
    radius = size * 0.45

    def pixel(x: int, y: int) -> tuple[int, int, int, int]:
        inside = (x - center) ** 2 + (y - center) ** 2 <= radius**2
        red = x * 255 // max(size - 1, 1)
        green = y * 255 // max(size - 1, 1)
        return (red, green, 255 - red // 2, 255) if inside else (0, 0, 0, 0)

    return pixel


def bgra(rgba_pixel):
    """The same pixel function with bitmap channel order."""

    def pixel(x: int, y: int) -> tuple[int, int, int, int]:
        red, green, blue, alpha = rgba_pixel(x, y)
        return (blue, green, red, alpha)

    return pixel


def transparent_outside_disc(size: int):
    rgba = disc(size)
    return lambda x, y: rgba(x, y)[3] == 0


def legacy_icon() -> bytes:
    sixteen_colors = [(r * 255, g * 255, b * 255) for r in (0, 1) for g in (0, 1) for b in (0, 1)]
    sixteen_colors += [(r * 128, g * 128, b * 128) for r in (0, 1) for g in (0, 1) for b in (0, 1)]

    def top_left_corner(x: int, y: int) -> bool:
        return x < 4 and y < 4

    def palette_index(x: int, y: int) -> int:
        # Black (index 0) under the set mask bits keeps the corner transparent; any other color
        # would make Windows XOR it into the screen
        return 0 if top_left_corner(x, y) else (x // 2 + y // 2) % 16

    return container(
        1,
        [
            (16, 16, 0, 0, bitmap(16, 4, sixteen_colors, palette_index, top_left_corner)),
            (32, 0, 1, 32, bitmap(32, 32, [], bgra(disc(32)), transparent_outside_disc(32))),
            (256, 0, 1, 32, png_rgba(256, disc(256))),
        ],
    )


def legacy_cursor() -> bytes:
    return container(
        2,
        [
            (32, 0, 5, 7, bitmap(32, 32, [], bgra(disc(32)), transparent_outside_disc(32))),
            (256, 0, 40, 56, png_rgba(256, disc(256))),
        ],
    )


def main() -> int:
    outputs = {
        FIXTURES / "icon" / "source" / "legacy.ico": legacy_icon(),
        FIXTURES / "icon" / "source" / "layer_16.png": png_rgba(16, disc(16)),
        FIXTURES / "icon" / "source" / "layer_32.png": png_rgba(32, disc(32)),
        FIXTURES / "icon" / "source" / "layer_256.png": png_rgba(256, disc(256)),
        FIXTURES / "cursor" / "source" / "legacy.cur": legacy_cursor(),
    }
    for path, data in outputs.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        print(f"{path.relative_to(FIXTURES).as_posix()}: {len(data)} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
