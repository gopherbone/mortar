# SPDX-License-Identifier: GPL-2.0-or-later
"""Compose the Mortar showcase video from captured frames.

Input: the capture (tools/showcase/capture.nix: clips/<name>/NNNN.png at
30 fps, clips/<name>/events.tsv, sfx/*.wav, fonts/), and optionally a
directory of device clips in the same layout (tools/showcase/device.sh).
Output: an H.264 MP4 with a soundtrack made of the shell's own UI sounds
(placed on the frames where the shell played them) over a small
synthesized beat.

    python3 compose.py --capture DIR [--device DIR] --cut rough|full \
        --layout wide|square --out FILE.mp4

Pond style (notes/ui/style.md): flat blocks, 1 px ink lines (scaled up
here), Instrument Serif italic for the big words, Onest and Geist Mono.
"""

import argparse
import math
import os
import subprocess
import sys
import wave
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

FPS = 30
RATE = 44100
BPM = 120
BEAT = 60 / BPM  # 0.5 s = 15 frames

GROUND = (0xE1, 0xEB, 0xF7)
GROUND2 = (0xBF, 0xCB, 0xD8)
INK = (0x1D, 0x1D, 0x1D)
SOFT = (0x4C, 0x51, 0x64)
FAINT = (0x9B, 0xA7, 0xB5)
LIME = (0xCE, 0xF0, 0x58)
LIME_INK = (0x1B, 0x32, 0x2D)
LAV = (0xCB, 0xA6, 0xF7)
LAV_INK = (0x3F, 0x1D, 0x68)
YELLOW = (0xFF, 0xD3, 0x30)
ORANGE = (0xFF, 0x9F, 0x67)


# --------------------------------------------------------------- storyboard

@dataclass
class Shot:
    """A run of frames from a clip: frame i shows clip frame start + i*speed."""
    clip: str
    start: int
    n: int
    speed: float = 1.0
    kicker: str = ""
    title: str = ""
    body: str = ""
    accent: tuple = LIME
    wipe: tuple | None = None  # a block of this colour wipes off at the start
    sfx: list = field(default_factory=list)  # extra (frame, name)
    keys: bool = True  # show the buttons pressed


@dataclass
class Card:
    """A full-frame flat title card."""
    n: int
    title: str
    sub: str = ""
    bg: tuple = LIME
    fg: tuple = INK
    kind: str = "title"  # title | end
    sfx: list = field(default_factory=list)


def storyboard(cut):
    if cut == "rough":
        return [
            Card(60, "Mortar", "NixOS for the TrimUI Brick", sfx=[(0, "open")]),
            Shot("boot", 0, 90, kicker="01 · Boot", title="Hello, Brick.",
                 body="Mainline Linux and NixOS,\nwith a shell of its own.", accent=LIME, wipe=LIME),
            Shot("home", 0, 75, kicker="02 · Home", title="Springs, sounds\nand haptics.",
                 body="Every move has a spring,\na synthesized sound and a buzz.", accent=LAV),
            Shot("launch-ai", 0, 30, kicker="03 · Launch", title="Apps grow out\nof their tiles.",
                 body="Fullscreen Wayland apps,\nand they keep running.", accent=YELLOW),
            Shot("ai-ask", 0, 36, speed=2.0, kicker="04 · Claude", title="Claude Code,\non a controller.",
                 body="It asks before it acts:\nA allows, B denies.", accent=ORANGE),
            Shot("ai-ask", 100, 50, kicker="04 · Claude", title="Claude Code,\non a controller.",
                 body="It asks before it acts:\nA allows, B denies.", accent=ORANGE),
            Shot("ai-ask", 150, 40, speed=1.5, kicker="04 · Claude", title="Claude Code,\non a controller.",
                 body="It asks before it acts:\nA allows, B denies.", accent=ORANGE),
            Shot("menu", 0, 118, kicker="05 · Menu", title="One menu over\neverything.",
                 body="Running apps with live previews,\nlaunchers, quick settings, power.", accent=LIME),
            Shot("quick", 18, 88, kicker="06 · Quick", title="Quick settings.",
                 body="Volume and brightness\nwithout leaving the app.", accent=LAV),
            Card(45, "Mortar", "Built with Nix · runs on a Brick", bg=INK, fg=GROUND, kind="end", sfx=[(0, "chime")]),
        ]
    raise SystemExit(f"unknown cut {cut}")


# ------------------------------------------------------------------ helpers

class Fonts:
    def __init__(self, d: Path):
        self.d = d
        self.cache = {}

    def get(self, name, size):
        files = {
            "serif": "InstrumentSerif-Italic.ttf",
            "serif-roman": "InstrumentSerif-Regular.ttf",
            "body": "Onest-Medium.ttf",
            "body-bold": "Onest-SemiBold.ttf",
            "mono": "GeistMono-SemiBold.ttf",
            "mono-light": "GeistMono-Regular.ttf",
        }
        key = (name, size)
        if key not in self.cache:
            f = self.d / files[name]
            if not f.exists():  # fall back to whatever the capture shipped
                cands = sorted(self.d.glob(files[name].split("-")[0] + "*"))
                f = cands[0]
            self.cache[key] = ImageFont.truetype(str(f), size)
        return self.cache[key]


def ease_out(t):
    """cubic-bezier(.22,1,.36,1)-ish: a quintic ease-out."""
    t = min(max(t, 0.0), 1.0)
    return 1 - (1 - t) ** 5


def spring(t, k=320.0, zeta=0.62):
    """The shell's Bouncy spring from 0 to 1, t in seconds."""
    w0 = math.sqrt(k)
    if zeta >= 1:
        return 1 - (1 + w0 * t) * math.exp(-w0 * t)
    wd = w0 * math.sqrt(1 - zeta * zeta)
    return 1 - math.exp(-zeta * w0 * t) * (math.cos(wd * t) + zeta * w0 / wd * math.sin(wd * t))


def load_events(clipdir: Path):
    ev = []
    p = clipdir / "events.tsv"
    if p.exists():
        for line in p.read_text().splitlines():
            parts = line.split("\t")
            if len(parts) == 3:
                ev.append((int(parts[0]), parts[1], parts[2]))
    return ev


def sfx_file(name):
    """'Move' / 'DrawerUp' / 'Vol(70)' -> the capture's wav name."""
    if name.startswith("Vol("):
        return "vol" + name[4:-1]
    return name.lower()


# ------------------------------------------------------------------- layout

class Layout:
    def __init__(self, kind, fonts: Fonts):
        self.kind = kind
        self.f = fonts
        if kind == "wide":
            self.W, self.H = 1920, 1080
            self.scale = 0.9
            sw, sh = round(1024 * self.scale), round(768 * self.scale)
            bez, pad = 40, 38
            bw = sw + 2 * (bez + pad)
            self.body = (self.W - 104 - bw, 60, self.W - 104, self.H + 200)
            self.cap = (110, 250, 700)  # x, y, width of the caption column
        else:
            self.W, self.H = 1080, 1080
            self.scale = 0.78
            sw, sh = round(1024 * self.scale), round(768 * self.scale)
            bez, pad = 34, 32
            bw = sw + 2 * (bez + pad)
            x0 = (self.W - bw) // 2
            self.body = (x0, 36, x0 + bw, self.H + 200)
            self.cap = (64, 790, self.W - 128)
        self.sw, self.sh = sw, sh
        bx0, by0 = self.body[0], self.body[1]
        self.screen = (bx0 + pad + bez, by0 + pad + bez)
        self.bezel = (bx0 + pad, by0 + pad, bx0 + pad + sw + 2 * bez, by0 + pad + sh + 2 * bez)
        self.device = self._device()

    def _device(self):
        """The Brick, flat: body, bezel, D-pad, face buttons (cut off below)."""
        bx0, by0, bx1, by1 = self.body
        w, h = bx1 - bx0 + 8, by1 - by0 + 8
        im = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        d = ImageDraw.Draw(im)
        o = 4
        r = 56 if self.kind == "wide" else 46
        lw = 4
        d.rounded_rectangle((o, o, w - o - 1, h - o - 1), r, fill=GROUND2 + (255,), outline=INK, width=lw)
        x0, y0, x1, y1 = (v - bx0 + o if i % 2 == 0 else v - by0 + o for i, v in enumerate(self.bezel))
        d.rounded_rectangle((x0, y0, x1, y1), 22, fill=INK + (255,))
        # controls under the screen
        cy = y1 + (150 if self.kind == "wide" else 120)
        s = 1.0 if self.kind == "wide" else 0.82
        dx = x0 + 150 * s
        arm, thick = 78 * s, 50 * s
        d.rounded_rectangle((dx - arm, cy - thick / 2, dx + arm, cy + thick / 2), 8, fill=INK + (255,))
        d.rounded_rectangle((dx - thick / 2, cy - arm, dx + thick / 2, cy + arm), 8, fill=INK + (255,))
        ax = x1 - 150 * s
        br = 30 * s
        for (ox, oy), col in zip([(0, -62), (62, 0), (0, 62), (-62, 0)], [LAV, ORANGE, LIME, YELLOW]):
            cx, cyy = ax + ox * s, cy + oy * s
            d.ellipse((cx - br, cyy - br, cx + br, cyy + br), fill=col + (255,), outline=INK, width=lw)
        # MENU / SELECT / START pills
        mx = (x0 + x1) / 2
        for i, dxp in enumerate((-70, 0, 70)):
            px = mx + dxp * s
            d.rounded_rectangle((px - 24 * s, cy + 40 * s, px + 24 * s, cy + 56 * s), 8 * s,
                                fill=(SOFT if i != 1 else INK) + (255,))
        # speaker slots
        for i in range(4):
            sx = mx - 45 * s + i * 30 * s
            d.rounded_rectangle((sx - 4 * s, cy - 40 * s, sx + 4 * s, cy - 6 * s), 4 * s, fill=SOFT + (255,))
        return im


# ----------------------------------------------------------------- renderer

KEY_LABEL = {"menu": "MENU", "start": "START", "select": "SELECT", "r1": "R1", "l1": "L1",
             "a": "A", "b": "B", "x": "X", "y": "Y", "left": "◀", "right": "▶", "up": "▲", "down": "▼"}
KEY_FILL = {"a": LIME, "b": ORANGE, "x": LAV, "y": YELLOW}


class Renderer:
    def __init__(self, layout: Layout, roots):
        self.L = layout
        self.roots = roots
        self.events = {}
        self.cap_cache = {}

    def clipdir(self, clip):
        for r in self.roots:
            p = r / "clips" / clip
            if p.is_dir():
                return p
        raise SystemExit(f"no clip {clip} in {', '.join(map(str, self.roots))}")

    def clip_events(self, clip):
        if clip not in self.events:
            self.events[clip] = load_events(self.clipdir(clip))
        return self.events[clip]

    def frame_count(self, clip):
        return len(list(self.clipdir(clip).glob("*.png")))

    def screen_frame(self, clip, f):
        n = self.frame_count(clip)
        f = max(0, min(int(f), n - 1))
        im = Image.open(self.clipdir(clip) / f"{f:04d}.png").convert("RGB")
        if im.size != (self.L.sw, self.L.sh):
            im = im.resize((self.L.sw, self.L.sh), Image.LANCZOS)
        return im

    # captions ----------------------------------------------------------
    def caption(self, img, shot: Shot, t_in):
        L, F = self.L, self.L.f
        x, y, w = L.cap
        d = ImageDraw.Draw(img)
        big = 96 if L.kind == "wide" else 64
        if L.kind == "square":
            d.rectangle((0, y - 40, L.W, L.H), fill=GROUND)
            d.line((0, y - 40, L.W, y - 40), fill=INK, width=3)
        a1 = ease_out(t_in / 0.35)
        a2 = ease_out((t_in - 0.08) / 0.35)
        a3 = ease_out((t_in - 0.16) / 0.35)
        # kicker: a square in the accent, mono caps
        if shot.kicker:
            ky = y + (1 - a1) * 20
            sq = 22 if L.kind == "wide" else 18
            d.rectangle((x, ky + 4, x + sq, ky + 4 + sq), fill=shot.accent, outline=INK, width=2)
            d.text((x + sq + 14, ky), shot.kicker.upper(), font=F.get("mono", 26 if L.kind == "wide" else 22),
                   fill=self.mix(GROUND, SOFT, a1))
        ty = y + (56 if L.kind == "wide" else 40)
        if L.kind == "square":
            # one line title at left, body under it
            d.text((x, ty + (1 - a2) * 28), shot.title.replace("\n", " "), font=F.get("serif", big),
                   fill=self.mix(GROUND, INK, a2))
            if shot.body:
                d.text((x, ty + big + 14 + (1 - a3) * 20), shot.body.replace("\n", " "),
                       font=F.get("body", 26), fill=self.mix(GROUND, SOFT, a3))
            return
        lines = shot.title.split("\n")
        for i, line in enumerate(lines):
            d.text((x, ty + i * big * 1.0 + (1 - a2) * 28), line, font=F.get("serif", big),
                   fill=self.mix(GROUND, INK, a2))
        by = ty + len(lines) * big + 36
        if shot.body:
            for i, line in enumerate(shot.body.split("\n")):
                d.text((x, by + i * 42 + (1 - a3) * 20), line, font=F.get("body", 31),
                       fill=self.mix(GROUND, SOFT, a3))

    @staticmethod
    def mix(a, b, t):
        t = min(max(t, 0.0), 1.0)
        return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))

    # key cast ----------------------------------------------------------
    def keys(self, img, presses, now):
        """The last few buttons pressed, newest lime-ish, popping in."""
        L, F = self.L, self.L.f
        recent = [(f, k) for f, k in presses if f <= now and now - f < 40][-4:]
        if not recent:
            return
        d = ImageDraw.Draw(img)
        if L.kind == "wide":
            x, y, h = L.cap[0], 880, 64
        else:
            x, y, h = L.W - 64 - 4 * 74, L.cap[1] - 6, 56
            x = L.W - 64
        chips = []
        for f, k in recent:
            age = (now - f) / FPS
            pop = spring(age, 620, 0.55)
            fade = 1.0 if now - f < 28 else max(0.0, 1 - (now - f - 28) / 12)
            chips.append((k, pop, fade, f == recent[-1][0] and now - f < 10))
        font = F.get("mono", 28 if L.kind == "wide" else 24)
        cx = x
        if L.kind == "square":  # right aligned
            total = sum(self.chip_w(k, h, font) + 12 for k, *_ in chips)
            cx = x - total
        for k, pop, fade, hot in chips:
            cw = self.chip_w(k, h, font)
            s = 0.6 + 0.4 * pop
            fill = KEY_FILL.get(k, LIME if hot else GROUND2)
            fill = self.mix(GROUND, fill, fade)
            line = self.mix(GROUND, INK, fade)
            w2, h2 = cw * s / 2, h * s / 2
            mx, my = cx + cw / 2, y + h / 2
            box = (mx - w2, my - h2, mx + w2, my + h2)
            if k in ("a", "b", "x", "y"):
                d.ellipse(box, fill=fill, outline=line, width=3)
            elif k in ("left", "right", "up", "down"):
                d.rectangle(box, fill=fill, outline=line, width=3)
                self.arrow(d, mx, my, h2 * 0.5, k, line)
            else:
                d.rounded_rectangle(box, h2, fill=fill, outline=line, width=3)
            if k not in ("left", "right", "up", "down"):
                lab = KEY_LABEL.get(k, k.upper())
                fs = F.get("mono", max(8, int((28 if L.kind == "wide" else 24) * s)))
                tw = d.textlength(lab, font=fs)
                d.text((mx - tw / 2, my - fs.size * 0.62), lab, font=fs, fill=line)
            cx += cw + 12

    @staticmethod
    def chip_w(k, h, font):
        if k in ("a", "b", "x", "y", "left", "right", "up", "down"):
            return h
        return int(font.getlength(KEY_LABEL.get(k, k.upper())) + h * 0.8)

    @staticmethod
    def arrow(d, x, y, r, k, col):
        pts = {"right": [(x - r * 0.7, y - r), (x - r * 0.7, y + r), (x + r, y)],
               "left": [(x + r * 0.7, y - r), (x + r * 0.7, y + r), (x - r, y)],
               "up": [(x - r, y + r * 0.7), (x + r, y + r * 0.7), (x, y - r)],
               "down": [(x - r, y - r * 0.7), (x + r, y - r * 0.7), (x, y + r)]}[k]
        d.polygon(pts, fill=col)

    # frames ------------------------------------------------------------
    def shot_frame(self, shot: Shot, i, t_in, shake):
        L = self.L
        img = Image.new("RGB", (L.W, L.H), GROUND)
        d = ImageDraw.Draw(img)
        bx0, by0, bx1, by1 = L.body
        # a flat accent block behind the Brick
        if L.kind == "wide":
            d.rectangle((bx0 - 72, -10, L.W + 10, L.H + 10), fill=shot.accent, outline=INK, width=3)
        else:
            d.rectangle((-10, 150, L.W + 10, L.cap[1] - 40), fill=shot.accent, outline=INK, width=3)
        sx = int(round(shake))
        img.paste(L.device, (bx0 - 4 + sx, by0 - 4), L.device)
        f = shot.start + i * shot.speed
        img.paste(self.screen_frame(shot.clip, f), (L.screen[0] + sx, L.screen[1]))
        self.caption(img, shot, t_in)
        return img

    def card_frame(self, card: Card, i):
        L, F = self.L, self.L.f
        img = Image.new("RGB", (L.W, L.H), card.bg)
        d = ImageDraw.Draw(img)
        t = i / FPS
        big = 300 if L.kind == "wide" else 230
        font = F.get("serif", big)
        tw = d.textlength(card.title, font=font)
        a = ease_out(t / 0.5)
        x = (L.W - tw) / 2
        y = L.H / 2 - big * 0.72 + (1 - a) * 60
        # four blocks fly in and settle into a square (the boot logo)
        s = 46 if L.kind == "wide" else 38
        cols = [LIME if card.bg != LIME else INK, LAV, YELLOW, ORANGE]
        bx, byy = L.W / 2 - s, y - s * 2 - 30
        if card.kind == "end":
            cols = [LIME, LAV, YELLOW, ORANGE]
        for k, (ox, oy) in enumerate([(0, 0), (1, 0), (0, 1), (1, 1)]):
            p = spring(max(0.0, t - 0.06 * k), 320, 0.62)
            fx = bx + ox * s + (1 - p) * (ox - 0.5) * 380
            fy = byy + oy * s + (1 - p) * (oy - 0.5) * 260
            d.rectangle((fx, fy, fx + s, fy + s), fill=cols[k], outline=card.fg if card.bg == INK else INK, width=3)
        d.text((x, y), card.title, font=font, fill=self.mix(card.bg, card.fg, a))
        if card.sub:
            a2 = ease_out((t - 0.2) / 0.5)
            sf = F.get("mono", 34 if L.kind == "wide" else 28)
            sw = d.textlength(card.sub.upper(), font=sf)
            d.text(((L.W - sw) / 2, y + big * 1.12 + (1 - a2) * 20), card.sub.upper(), font=sf,
                   fill=self.mix(card.bg, card.fg, a2))
        return img


# -------------------------------------------------------------------- audio

def load_wav(p: Path):
    with wave.open(str(p)) as w:
        n = w.getnframes()
        data = np.frombuffer(w.readframes(n), dtype=np.int16).astype(np.float32) / 32768
        if w.getnchannels() == 2:
            data = data.reshape(-1, 2).mean(axis=1)
        if w.getframerate() != RATE:
            raise SystemExit(f"{p}: rate {w.getframerate()}")
    return data


def beat(seconds, seed=7):
    """A small, quiet beat in C major (the UI sounds are C-E-G-C): kick,
    hats, a clap on 2 and 4, a triangle bass on C-Am-F-G and soft chord
    stabs; the kick ducks the rest."""
    rng = np.random.default_rng(seed)
    n = int(seconds * RATE)
    out = np.zeros((n, 2), np.float32)
    duck = np.ones(n, np.float32)
    beat_n = int(BEAT * RATE)

    def add(sig, at, pan=0.0, gain=1.0):
        a = int(at * RATE)
        if a >= n:
            return
        sig = sig[: n - a] * gain
        out[a:a + len(sig), 0] += sig * (1 - pan) * 0.5 * 2 ** 0.5
        out[a:a + len(sig), 1] += sig * (1 + pan) * 0.5 * 2 ** 0.5

    t = np.arange(int(0.45 * RATE)) / RATE
    kick = np.sin(2 * np.pi * (45 * t + (110 / 18) * (1 - np.exp(-18 * t)))) * np.exp(-t * 7)
    kick += 0.3 * np.sin(2 * np.pi * 1800 * t) * np.exp(-t * 300)
    th = np.arange(int(0.05 * RATE)) / RATE
    hat = rng.standard_normal(len(th)).astype(np.float32)
    hat = np.diff(hat, prepend=0) * np.exp(-th * 90)
    tc = np.arange(int(0.25 * RATE)) / RATE
    clap = rng.standard_normal(len(tc)).astype(np.float32)
    clap = np.convolve(clap, np.ones(6) / 6, mode="same") - np.convolve(clap, np.ones(40) / 40, mode="same")
    clap *= np.exp(-tc * 22) * (1 + 0.6 * (tc < 0.012))

    def tri(f, d, g=1.0, decay=4.0):
        tt = np.arange(int(d * RATE)) / RATE
        ph = (f * tt) % 1.0
        s = 4 * np.abs(ph - 0.5) - 1
        env = np.minimum(1, tt / 0.005) * np.exp(-tt * decay)
        return (s * env * g).astype(np.float32)

    def sine(f, d, g=1.0, decay=3.0):
        tt = np.arange(int(d * RATE)) / RATE
        env = np.minimum(1, tt / 0.01) * np.exp(-tt * decay)
        return (np.sin(2 * np.pi * f * tt) * env * g).astype(np.float32)

    hz = lambda m: 440 * 2 ** ((m - 69) / 12)
    prog = [(48, [60, 64, 67, 71]), (45, [57, 60, 64, 67]), (41, [57, 60, 65, 69]), (43, [55, 59, 62, 67])]
    bars = int(seconds / (4 * BEAT)) + 1
    for b in range(bars):
        root, chord = prog[b % 4]
        for q in range(4):
            at = (b * 4 + q) * BEAT
            add(kick, at, 0, 0.55)
            dn = int(at * RATE)
            env = 1 - 0.55 * np.exp(-np.arange(beat_n) / (0.09 * RATE))
            duck[dn:dn + beat_n] = env[: max(0, min(beat_n, n - dn))]
            if q in (1, 3):
                add(clap, at, 0.1, 0.22)
            for e in range(2):
                add(hat, at + e * BEAT / 2 + BEAT / 4, -0.3 + 0.6 * e, 0.10 if e else 0.07)
            # bass: root on the beat, octave on the off-beat
            add(tri(hz(root), BEAT * 0.9, 0.30, 5), at, 0)
            add(tri(hz(root + 12), BEAT * 0.4, 0.12, 9), at + BEAT / 2, 0)
        # chord stabs on the "and" of 1 and 3, a pluck arpeggio on top
        for q in (0, 2):
            for k, m in enumerate(chord):
                add(sine(hz(m), 0.6, 0.045, 5), (b * 4 + q) * BEAT + BEAT / 2, -0.4 + 0.27 * k)
        for k in range(8):
            m = chord[(k * 3) % 4] + 12
            add(sine(hz(m), 0.3, 0.025, 11), (b * 4) * BEAT + k * BEAT / 2 + BEAT / 4, 0.5 if k % 2 else -0.5)
    out[:, 0] *= np.maximum(duck, 0.45)
    out[:, 1] *= np.maximum(duck, 0.45)
    return out


# --------------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--capture", required=True, type=Path)
    ap.add_argument("--device", type=Path)
    ap.add_argument("--cut", default="rough")
    ap.add_argument("--layout", default="wide", choices=["wide", "square"])
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--stills", type=Path, help="also save these frames here (comma list via --at)")
    ap.add_argument("--at", default="")
    ap.add_argument("--no-music", action="store_true")
    a = ap.parse_args()

    fonts = Fonts(a.capture / "fonts")
    L = Layout(a.layout, fonts)
    roots = [a.capture] + ([a.device] if a.device else [])
    R = Renderer(L, roots)
    board = storyboard(a.cut)
    total = sum(s.n for s in board)
    secs = total / FPS
    print(f"{a.cut}/{a.layout}: {len(board)} shots, {total} frames, {secs:.1f} s", file=sys.stderr)

    # sound: events -> (time, wav)
    cues = []  # (frame, name, gain)
    haptics = []  # (frame, kind)
    t0 = 0
    prev_cap = None
    starts = []
    for s in board:
        starts.append(t0)
        for f, name in s.sfx:
            cues.append((t0 + f, name, 1.0))
        if isinstance(s, Shot):
            for f, kind, v in R.clip_events(s.clip):
                i = (f - s.start) / s.speed
                if 0 <= i < s.n:
                    if kind == "sfx":
                        cues.append((t0 + i, sfx_file(v), 1.0))
                    elif kind == "haptic":
                        haptics.append((t0 + i, v))
        t0 += s.n

    # video
    a.out.parent.mkdir(parents=True, exist_ok=True)
    wav_path = a.out.with_suffix(".wav")
    mix = np.zeros((int(secs * RATE) + RATE, 2), np.float32)
    if not a.no_music:
        mix[: int(secs * RATE)] += beat(secs) * 0.55
    sfx_dir = a.capture / "sfx"
    cache = {}
    for f, name, g in cues:
        if name not in cache:
            p = sfx_dir / f"{name}.wav"
            if not p.exists():
                print(f"no sound {name}", file=sys.stderr)
                cache[name] = None
                continue
            cache[name] = load_wav(p)
        s = cache[name]
        if s is None:
            continue
        at = int(f / FPS * RATE)
        seg = s[: max(0, len(mix) - at)] * 1.6 * g
        mix[at:at + len(seg)] += seg[:, None]
    # end: fade the last half second
    end = int(secs * RATE)
    fade = int(0.6 * RATE)
    mix[end - fade:end] *= np.linspace(1, 0, fade)[:, None]
    mix = mix[:end]
    peak = np.abs(mix).max() or 1
    mix *= 0.89 / peak
    with wave.open(str(wav_path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes((mix * 32767).astype(np.int16).tobytes())

    stills = {int(x) for x in a.at.split(",") if x}
    if a.stills:
        a.stills.mkdir(parents=True, exist_ok=True)
    ff = subprocess.Popen([
        "ffmpeg", "-y", "-loglevel", "error",
        "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{L.W}x{L.H}", "-r", str(FPS), "-i", "-",
        "-i", str(wav_path),
        "-c:v", "libx264", "-preset", "slow", "-crf", "17", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart", str(a.out)],
        stdin=subprocess.PIPE)
    g = 0
    cap_since = 0
    for si, s in enumerate(board):
        same_cap = isinstance(s, Shot) and prev_cap == (s.kicker, s.title)
        if not same_cap:
            cap_since = g
        presses = []
        if isinstance(s, Shot) and s.keys:
            # presses from earlier shots of the same caption stay on screen
            for sj in range(si, -1, -1):
                o = board[sj]
                if not (isinstance(o, Shot) and (o.kicker, o.title) == (s.kicker, s.title)):
                    break
                for f, kind, v in R.clip_events(o.clip):
                    i = (f - o.start) / o.speed
                    if kind == "press" and 0 <= i < o.n:
                        presses.append((starts[sj] + i, v))
            presses.sort()
        for i in range(s.n):
            if isinstance(s, Card):
                img = R.card_frame(s, i)
            else:
                shake = 0.0
                for hf, kind in haptics:
                    age = g - hf
                    dur, amp = {"Tick": (3, 3), "Double": (6, 5), "Long": (10, 6)}.get(kind, (3, 3))
                    if 0 <= age < dur:
                        shake += amp * (1 if int(age) % 2 == 0 else -1) * (1 - age / dur)
                img = R.shot_frame(s, i, (g - cap_since) / FPS, shake)
                if s.keys:
                    R.keys(img, presses, g)
                if s.wipe and i < 12:
                    p = ease_out(i / 11)
                    ImageDraw.Draw(img).rectangle((p * L.W, 0, L.W, L.H), fill=s.wipe)
            ff.stdin.write(img.tobytes())
            if g in stills:
                img.save(a.stills / f"frame-{g:04d}.png")
            g += 1
        prev_cap = (s.kicker, s.title) if isinstance(s, Shot) else None
    ff.stdin.close()
    ff.wait()
    print(a.out, file=sys.stderr)


if __name__ == "__main__":
    main()
