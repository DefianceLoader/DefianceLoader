"""Decode the DDS textures the companion mods derive from: uncompressed 32-bit
RGBA, and BC7 in its single-subset modes (4, 5 and 6), which the game's
gradients use. Other BC7 modes raise ValueError rather than guess."""
import struct

WEIGHTS = {2: [0, 21, 43, 64], 3: [0, 9, 18, 27, 37, 46, 55, 64],
           4: [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64]}
# DXGI_FORMAT_BC7_UNORM and _UNORM_SRGB.
BC7_FORMATS = (98, 99)


class Bits:
    def __init__(self, block):
        self.value = int.from_bytes(block, "little")
        self.at = 0

    def take(self, count):
        out = (self.value >> self.at) & ((1 << count) - 1)
        self.at += count
        return out


def expand(value, bits):
    """An n-bit endpoint widened to 8 bits by repeating its high bits."""
    value <<= 8 - bits
    return value | (value >> bits)


def interpolate(first, second, index, bits):
    weight = WEIGHTS[bits][index]
    return ((64 - weight) * first + weight * second + 32) >> 6


def indices(bits, width):
    """Sixteen indices; the first (the anchor) is one bit shorter."""
    return [bits.take(width - 1 if i == 0 else width) for i in range(16)]


def block(data):
    """The 16 RGBA pixels, row by row, of one 16-byte BC7 block."""
    bits = Bits(data)
    mode = 0
    while mode < 8 and not bits.take(1):
        mode += 1
    if mode == 6:
        channels = [[bits.take(7) for _ in range(2)] for _ in range(4)]
        parity = [bits.take(1), bits.take(1)]
        ends = [[(channels[c][e] << 1) | parity[e] for c in range(4)] for e in range(2)]
        return [tuple(interpolate(ends[0][c], ends[1][c], i, 4) for c in range(4))
                for i in indices(bits, 4)]
    if mode in (4, 5):
        rotation = bits.take(2)
        swap = bits.take(1) if mode == 4 else 0
        colour_bits, alpha_bits = (5, 6) if mode == 4 else (7, 8)
        colour = [[expand(bits.take(colour_bits), colour_bits) for _ in range(2)] for _ in range(3)]
        alpha = [expand(bits.take(alpha_bits), alpha_bits) for _ in range(2)]
        if mode == 4:
            short, long = indices(bits, 2), indices(bits, 3)
            (ci, cw), (ai, aw) = ((short, 2), (long, 3)) if not swap else ((long, 3), (short, 2))
        else:
            (ci, cw), (ai, aw) = (indices(bits, 2), 2), (indices(bits, 2), 2)
        out = []
        for k in range(16):
            pixel = [interpolate(colour[c][0], colour[c][1], ci[k], cw) for c in range(3)]
            pixel.append(interpolate(alpha[0], alpha[1], ai[k], aw))
            if rotation:
                pixel[3], pixel[rotation - 1] = pixel[rotation - 1], pixel[3]
            out.append(tuple(pixel))
        return out
    raise ValueError(f"BC7 mode {mode} is not supported")


def decode(data):
    """(width, height, rows of RGBA tuples) of a DDS texture's top mip."""
    if data[:4] != b"DDS " or len(data) < 128:
        raise ValueError("not a DDS texture")
    header = struct.unpack("<31I", data[4:128])
    height, width = header[2], header[3]
    fourcc, bit_count = data[84:88], header[21]
    masks = header[22:26]
    if fourcc == b"DX10":
        if struct.unpack("<I", data[128:132])[0] not in BC7_FORMATS:
            raise ValueError("a DX10 texture that is not BC7")
        if width % 4 or height % 4:
            raise ValueError("a BC7 texture whose size is not a multiple of 4")
        body = data[148:]
        across = width // 4
        if len(body) < across * (height // 4) * 16:
            raise ValueError("a truncated BC7 texture")
        rows = [[None] * width for _ in range(height)]
        for by in range(height // 4):
            for bx in range(across):
                at = (by * across + bx) * 16
                for k, pixel in enumerate(block(body[at:at + 16])):
                    rows[by * 4 + k // 4][bx * 4 + k % 4] = pixel
        return width, height, rows
    if fourcc == b"\0\0\0\0" and bit_count == 32:
        shifts = []
        for mask in masks:
            if mask not in (0xff, 0xff00, 0xff0000, 0xff000000):
                raise ValueError("an uncompressed texture with unusual channel masks")
            shifts.append(mask.bit_length() - 8)
        body = data[128:]
        if len(body) < width * height * 4:
            raise ValueError("a truncated texture")
        rows = []
        for y in range(height):
            row = []
            for x in range(width):
                value = struct.unpack_from("<I", body, (y * width + x) * 4)[0]
                row.append(tuple((value >> shift) & 0xff for shift in shifts))
            rows.append(row)
        return width, height, rows
    raise ValueError("an unsupported DDS format")


def encode(rows):
    """An uncompressed 32-bit DDS of RGBA rows."""
    height, width = len(rows), len(rows[0])
    header = [124, 0x100f, height, width, width * 4, 0, 0] + [0] * 11
    header += [32, 0x41, 0, 32, 0xff0000, 0xff00, 0xff, 0xff000000]
    header += [0x1000, 0, 0, 0, 0]
    body = bytearray()
    for row in rows:
        for r, g, b, a in row:
            body += bytes([b, g, r, a])
    return b"DDS " + struct.pack("<31I", *header) + bytes(body)
