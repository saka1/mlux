#!/usr/bin/env python3
"""
KGP sub-cell offset (X, Y) の挙動を実験的に検証するスクリプト。

検証項目:
  Exp 1: 24px PNG, r=1, Y を変化 → Y はシフトか？スケーリングに影響するか？
  Exp 2: 24px PNG, r=2, Y を変化 → r=2 での挙動
  Exp 3: ch高PNG, r=1, Y を変化 → セル高さに一致する画像での挙動
  Exp 4: 1px PNG,  r/Y を変化   → 極端なスケーリングケース
  Exp 5: 24px PNG, r=1, src_h 指定 → ソース矩形 h がスケーリングに影響するか
  Exp 6: 24px PNG, r/c 未指定, Y を変化 → 自動サイズでの挙動
  Exp 7: X オフセット → 水平方向の挙動確認

使い方:
  python3 tools/kgp_offset_experiment.py
  Kitty / Ghostty / WezTerm など KGP 対応ターミナルで実行すること。
"""

import base64
import struct
import sys
import zlib
import fcntl
import termios


# ── PNG generation ──────────────────────────────────────────────

def make_png(width, height, r, g, b, a):
    """Create a minimal RGBA PNG in memory."""
    def chunk(ctype, data):
        c = ctype + data
        crc = struct.pack('>I', zlib.crc32(c) & 0xFFFFFFFF)
        return struct.pack('>I', len(data)) + c + crc

    sig = b'\x89PNG\r\n\x1a\n'
    ihdr = chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0))
    raw = b''
    row_data = bytes([r, g, b, a]) * width
    for _ in range(height):
        raw += b'\x00' + row_data
    idat = chunk(b'IDAT', zlib.compress(raw))
    iend = chunk(b'IEND', b'')
    return sig + ihdr + idat + iend


# ── Terminal helpers ────────────────────────────────────────────

def get_cell_size():
    buf = fcntl.ioctl(sys.stdout.fileno(), termios.TIOCGWINSZ, b'\x00' * 8)
    rows, cols, xpx, ypx = struct.unpack('HHHH', buf)
    if xpx == 0 or ypx == 0:
        return None, None, rows, cols
    return xpx // cols, ypx // rows, rows, cols


def move(row, col):
    sys.stdout.write(f'\x1b[{row};{col}H')


def clear():
    sys.stdout.write('\x1b[2J')
    sys.stdout.flush()


def write(s):
    sys.stdout.write(s)
    sys.stdout.flush()


# ── KGP primitives ─────────────────────────────────────────────

def kgp_upload(iid, png_data):
    b64 = base64.b64encode(png_data).decode()
    chunks = [b64[i:i + 4096] for i in range(0, len(b64), 4096)]
    for i, ch in enumerate(chunks):
        more = 1 if i < len(chunks) - 1 else 0
        if i == 0:
            write(f'\x1b_Ga=T,f=100,i={iid},q=2,m={more};{ch}\x1b\\')
        else:
            write(f'\x1b_Gm={more};{ch}\x1b\\')


def kgp_place(iid, **kw):
    """Place image. Keyword args: X, Y, c, r, w, h, z, C."""
    parts = [f'a=p', f'i={iid}', 'q=2']
    for k in ('X', 'Y', 'c', 'r', 'w', 'h', 'z', 'C'):
        if k in kw and kw[k]:
            parts.append(f'{k}={kw[k]}')
    write(f'\x1b_G{",".join(parts)}\x1b\\')


def kgp_delete_all():
    write('\x1b_Ga=d,d=A,q=2\x1b\\')


# ── Experiment runner ───────────────────────────────────────────

class Experiment:
    def __init__(self, cw, ch):
        self.cw = cw
        self.ch = ch
        self.row = 1
        self.IMG_COL = 40   # column where images are placed
        self.ids = {}
        self._next_id = 900

    def upload(self, name, png):
        self._next_id += 1
        iid = self._next_id
        self.ids[name] = iid
        kgp_upload(iid, png)
        return iid

    def header(self, text):
        self.row += 1
        move(self.row, 1)
        write(f'\x1b[1;33m{text}\x1b[0m')
        self.row += 1

    def place(self, label, img_name, extra_rows=1, **kw):
        """Place image with label. Shows reference text underneath."""
        iid = self.ids[img_name]
        move(self.row, 1)
        # Label (left side)
        write(f'  {label:36s}')
        # Reference text at image column
        move(self.row, self.IMG_COL)
        write('abcdefghij')
        # Place image on top of text
        move(self.row, self.IMG_COL)
        kgp_place(iid, **kw)
        self.row += extra_rows + 1

    def info(self, text):
        move(self.row, 1)
        write(f'  \x1b[90m{text}\x1b[0m')
        self.row += 1


def main():
    cw, ch, trows, tcols = get_cell_size()
    clear()

    if cw is None:
        print("ERROR: cell pixel size unknown. Use Kitty/Ghostty/WezTerm.")
        sys.exit(1)

    exp = Experiment(cw, ch)

    move(1, 1)
    write(f'\x1b[1mcell: {cw}x{ch} px  |  term: {tcols}x{trows}\x1b[0m')

    # ── Upload test images ──
    exp.upload('red24',   make_png(100, 24, 255, 50, 50, 160))   # 100x24 red
    exp.upload('blue_ch', make_png(100, ch, 50, 50, 255, 160))   # 100xCH blue
    exp.upload('green1',  make_png(100, 1,  50, 255, 50, 160))   # 100x1  green
    exp.upload('wide',    make_png(200, 24, 255, 150, 0, 160))   # 200x24 orange

    # ════════════════════════════════════════════════════════════
    # Exp 1: 24px PNG, r=1, varying Y
    # ════════════════════════════════════════════════════════════
    exp.header(f'Exp 1: 100x24 red PNG, r=1, varying Y  (ch={ch})')
    exp.info('Q: Does Y shift the image? Does it change visible height?')
    for y in [0, 2, 4, 8, ch // 4, ch // 2]:
        if y >= ch:
            continue
        exp.place(f'r=1  Y={y:2d}  (expect shift {y}px)', 'red24', r=1, Y=y if y else None)

    # ════════════════════════════════════════════════════════════
    # Exp 2: 24px PNG, r=2, varying Y
    # ════════════════════════════════════════════════════════════
    exp.header(f'Exp 2: 100x24 red PNG, r=2, varying Y  (ch={ch})')
    exp.info('Q: r=2 gives 2*ch display area. How does Y interact?')
    for y in [0, 4, ch // 2, ch - 1]:
        if y >= ch:
            continue
        exp.place(f'r=2  Y={y:2d}', 'red24', extra_rows=2, r=2, Y=y if y else None)

    # ════════════════════════════════════════════════════════════
    # Exp 3: ch-height PNG, r=1, varying Y
    # ════════════════════════════════════════════════════════════
    exp.header(f'Exp 3: 100x{ch} blue PNG (=ch), r=1, varying Y')
    exp.info('Q: No scaling needed. Pure Y offset behavior.')
    for y in [0, 2, 4, 8]:
        if y >= ch:
            continue
        exp.place(f'r=1  Y={y:2d}  (src=ch)', 'blue_ch', r=1, Y=y if y else None)

    # ════════════════════════════════════════════════════════════
    # Exp 4: 1px PNG, varying r and Y
    # ════════════════════════════════════════════════════════════
    exp.header('Exp 4: 100x1 green PNG — extreme scaling')
    exp.info('Q: 1px scaled to r*ch. How tall does it actually render?')
    for rv, y in [(1, 0), (1, 4), (2, 0)]:
        exp.place(f'r={rv}  Y={y:2d}  (1px src)', 'green1',
                  extra_rows=rv, r=rv, Y=y if y else None)

    # ════════════════════════════════════════════════════════════
    # Exp 5: source rect h — does it prevent scaling?
    # ════════════════════════════════════════════════════════════
    exp.header('Exp 5: 100x24 red PNG, r=1, varying src h')
    exp.info('Q: Does specifying h crop the source before scaling?')
    for sh in [24, 12, 6, 1]:
        exp.place(f'r=1  h={sh:2d}', 'red24', r=1, h=sh)

    # ════════════════════════════════════════════════════════════
    # Exp 6: no r/c specified (auto), varying Y
    # ════════════════════════════════════════════════════════════
    exp.header('Exp 6: 100x24 red PNG, NO r/c, varying Y')
    exp.info('Q: Auto-sized placement — does Y shift without resize?')
    for y in [0, 4, 8]:
        exp.place(f'auto  Y={y:2d}', 'red24', Y=y if y else None)

    # ════════════════════════════════════════════════════════════
    # Exp 7: X offset
    # ════════════════════════════════════════════════════════════
    exp.header('Exp 7: 200x24 orange PNG, r=1, varying X')
    exp.info('Q: X offset — shift or clip?')
    for x in [0, 4, cw // 2, cw - 1]:
        if x >= cw:
            continue
        exp.place(f'r=1  X={x:2d}  (cw={cw})', 'wide', r=1, X=x if x else None)

    # ════════════════════════════════════════════════════════════
    # Exp 8: Combined X+Y
    # ════════════════════════════════════════════════════════════
    exp.header('Exp 8: 100x24 red PNG, r=1, combined X+Y')
    for x, y in [(4, 4), (cw // 2, ch // 2), (0, ch // 2)]:
        if x >= cw or y >= ch:
            continue
        exp.place(f'r=1  X={x:2d}  Y={y:2d}', 'red24', r=1,
                  X=x if x else None, Y=y if y else None)

    # ── Footer ──
    exp.row += 1
    move(exp.row, 1)
    write('\x1b[1m--- END ---\x1b[0m')
    exp.row += 1
    move(exp.row, 1)
    write('Observe each row: does the colored block shift/stretch/clip vs "abcdefghij"?')
    exp.row += 1
    move(exp.row, 1)
    write('Press Enter to clean up...')
    sys.stdout.flush()

    input()
    kgp_delete_all()
    clear()
    move(1, 1)
    print('Cleaned up.')


if __name__ == '__main__':
    main()
