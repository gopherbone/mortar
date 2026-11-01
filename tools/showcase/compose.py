# SPDX-License-Identifier: GPL-2.0-or-later
"""Compose the Mortar showcase video from captured frames.

Input: one or more captures (tools/showcase/capture.nix: clips/<name>/
NNNN.png at 30 fps, clips/<name>/events.tsv, sfx/*.wav, fonts/). Output:
an H.264 MP4. The screen is the picture: real footage, long takes, on a
flat Pond ground with a thin ink bezel; a title at the start and the name
at the end. The only sound is the shell's own UI sounds, put on the frames
where the shell played them.

    python3 compose.py --capture DIR [--capture DIR ...] \
        --layout wide|square --out FILE.mp4 [--stills DIR --at 30,200]
"""

import argparse
import math
import subprocess
import sys
import wave
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

FPS = 30
RATE = 44100

GROUND = (0xE1, 0xEB, 0xF7)
INK = (0x1D, 0x1D, 0x1D)
SOFT = (0x4C, 0x51, 0x64)
LIME = (0xCE, 0xF0, 0x58)
LAV = (0xCB, 0xA6, 0xF7)
YELLOW = (0xFF, 0xD3, 0x30)
ORANGE = (0xFF, 0x9F, 0x67)


@dataclass
class Take:
    """Frames start .. start+n of a clip (n None: to the end)."""
    clip: str
    start: int = 0
    n: int | None = None


@dataclass
class Card:
    n: int
    title: str
    sub: str = ""


# Long takes of real use, in order. The cuts land where the shell itself
# changes scene (a launch, a switch), so it reads as one session.
BOARD = [
    Card(66, "Mortar", "NixOS for the TrimUI Brick"),
    Take("boot", 0, 96),
    Take("home"),
    Take("launch-ai"),
    Take("ai-ask", 0, 232),
    Take("menu", 0, 116),
    Take("launch-term"),
    Take("switch"),
    Take("quick"),
    Take("launch-pico8", 0, 58),   # MENU > Games > PICO-8 ...
    Take("launch-pico8", 96, 110),  # ... Splore boots (skips its blank start)
    Take("play", 0, 58),           # MENU > Games: mortar-play ...
    Take("play", 102, 58),         # ... its game list, a homebrew game
    Take("play", 372, 60),
    Take("choochoo", 0, 135),      # MENU > Music > ChooChooTracker
    Take("running", 12, 116),      # everything still running, live previews
    Take("close-home"),
    Take("sleep"),
    Take("night"),
    Card(84, "Mortar", "NixOS for the TrimUI Brick"),
]


class Fonts:
    FILES = {"serif": "InstrumentSerif-Italic.ttf", "mono": "GeistMono-SemiBold.ttf"}

    def __init__(self, d: Path):
        self.d, self.cache = d, {}

    def get(self, name, size):
        if (name, size) not in self.cache:
            self.cache[(name, size)] = ImageFont.truetype(str(self.d / self.FILES[name]), size)
        return self.cache[(name, size)]


def ease_out(t):
    t = min(max(t, 0.0), 1.0)
    return 1 - (1 - t) ** 5


def spring(t, k=320.0, zeta=0.62):
    """The shell's Bouncy spring, 0 -> 1."""
    w0 = math.sqrt(k)
    wd = w0 * math.sqrt(1 - zeta * zeta)
    return 1 - math.exp(-zeta * w0 * t) * (math.cos(wd * t) + zeta * w0 / wd * math.sin(wd * t))


def mix(a, b, t):
    t = min(max(t, 0.0), 1.0)
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


class Video:
    def __init__(self, layout, roots, fonts):
        self.roots, self.f = roots, fonts
        if layout == "wide":
            self.W, self.H, self.sw = 1920, 1080, 1280  # the screen at 1.25x
        else:
            self.W, self.H, self.sw = 1080, 1080, 960
        self.sh = self.sw * 3 // 4
        self.bezel = max(10, self.sw // 90)
        o = self.bezel * 2  # the offset block behind
        self.x = (self.W - self.sw - o) // 2
        self.y = (self.H - self.sh - o) // 2
        self.frames = {}

    def clipdir(self, clip):
        for r in self.roots:
            if (r / "clips" / clip).is_dir():
                return r / "clips" / clip
        raise SystemExit(f"no clip {clip}")

    def count(self, clip):
        if clip not in self.frames:
            self.frames[clip] = len(list(self.clipdir(clip).glob("*.png")))
        return self.frames[clip]

    def events(self, clip):
        p = self.clipdir(clip) / "events.tsv"
        out = []
        for line in p.read_text().splitlines() if p.exists() else []:
            f, kind, v = line.split("\t")
            out.append((int(f), kind, v))
        return out

    def take_frame(self, t: Take, i):
        im = Image.open(self.clipdir(t.clip) / f"{min(t.start + i, self.count(t.clip) - 1):04d}.png").convert("RGB")
        im = im.resize((self.sw, self.sh), Image.LANCZOS)
        img = Image.new("RGB", (self.W, self.H), GROUND)
        d = ImageDraw.Draw(img)
        b = self.bezel
        # a lavender block offset behind the screen, as behind a focused tile
        o = b * 2
        d.rectangle((self.x - b + o, self.y - b + o, self.x + self.sw + b + o, self.y + self.sh + b + o), fill=LAV, outline=INK, width=3)
        d.rounded_rectangle((self.x - b, self.y - b, self.x + self.sw + b, self.y + self.sh + b), b, fill=INK)
        img.paste(im, (self.x, self.y))
        return img

    def card_frame(self, c: Card, i, end):
        img = Image.new("RGB", (self.W, self.H), GROUND)
        d = ImageDraw.Draw(img)
        t = i / FPS
        # the opening card fades out into the boot screen (same ground)
        out = 1.0 if end else 1 - ease_out((t - (c.n / FPS - 0.4)) / 0.4)
        big = round(self.W * (0.15 if self.W > 1500 else 0.2))
        font = self.f.get("serif", big)
        a = ease_out(t / 0.6) * out
        tw = d.textlength(c.title, font=font)
        y = self.H / 2 - big * 0.62 + (1 - ease_out(t / 0.6)) * 40
        s = round(big * 0.16)
        bx, by = self.W / 2 - s, y - s * 2 - big * 0.08
        for k, (ox, oy, col) in enumerate([(0, 0, LIME), (1, 0, LAV), (0, 1, YELLOW), (1, 1, ORANGE)]):
            p = spring(max(0.0, t - 0.07 * k))
            fx = bx + ox * s + (1 - p) * (ox - 0.5) * s * 8
            fy = by + oy * s + (1 - p) * (oy - 0.5) * s * 6
            d.rectangle((fx, fy, fx + s, fy + s), fill=mix(GROUND, col, out), outline=mix(GROUND, INK, out), width=2)
        d.text(((self.W - tw) / 2, y), c.title, font=font, fill=mix(GROUND, INK, a))
        if c.sub:
            a2 = ease_out((t - 0.25) / 0.6) * out
            sf = self.f.get("mono", round(big * 0.12))
            sw = d.textlength(c.sub.upper(), font=sf)
            d.text(((self.W - sw) / 2, y + big * 1.1), c.sub.upper(), font=sf, fill=mix(GROUND, SOFT, a2))
        return img


def load_wav(p: Path):
    with wave.open(str(p)) as w:
        data = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768
        if w.getnchannels() == 2:
            data = data.reshape(-1, 2).mean(axis=1)
    return data


def sfx_name(v):
    """'Move' / 'DrawerUp' / 'Vol(70)' -> the capture's wav name."""
    return "vol" + v[4:-1] if v.startswith("Vol(") else v.lower()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--capture", required=True, type=Path, action="append")
    ap.add_argument("--layout", default="wide", choices=["wide", "square"])
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--stills", type=Path)
    ap.add_argument("--at", default="")
    a = ap.parse_args()

    V = Video(a.layout, a.capture, Fonts(a.capture[0] / "fonts"))
    board = [s if isinstance(s, Card) or s.n is not None else Take(s.clip, s.start, V.count(s.clip) - s.start) for s in BOARD]
    total = sum(s.n for s in board)
    secs = total / FPS
    print(f"{a.layout}: {len(board)} pieces, {total} frames, {secs:.1f} s", file=sys.stderr)

    # sound: the shell's UI sounds where it played them
    track = np.zeros(int(secs * RATE) + RATE, np.float32)
    cache = {}
    g = 0
    for s in board:
        if isinstance(s, Take):
            for f, kind, v in V.events(s.clip):
                if kind == "sfx" and s.start <= f < s.start + s.n:
                    name = sfx_name(v)
                    if name not in cache:
                        p = a.capture[0] / "sfx" / f"{name}.wav"
                        cache[name] = load_wav(p) if p.exists() else None
                    w = cache[name]
                    if w is not None:
                        at = int((g + f - s.start) / FPS * RATE)
                        seg = w[: len(track) - at]
                        track[at:at + len(seg)] += seg
        g += s.n
    track = track[: int(secs * RATE)]
    track *= 0.7 / max(np.abs(track).max(), 1e-6)
    a.out.parent.mkdir(parents=True, exist_ok=True)
    wav_path = a.out.with_suffix(".wav")
    with wave.open(str(wav_path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes((np.repeat(track[:, None], 2, axis=1) * 32767).astype(np.int16).tobytes())

    stills = {int(x) for x in a.at.split(",") if x}
    if a.stills:
        a.stills.mkdir(parents=True, exist_ok=True)
    ff = subprocess.Popen([
        "ffmpeg", "-y", "-loglevel", "error",
        "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{V.W}x{V.H}", "-r", str(FPS), "-i", "-",
        "-i", str(wav_path),
        "-c:v", "libx264", "-preset", "slow", "-crf", "16", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart", str(a.out)],
        stdin=subprocess.PIPE)
    g = 0
    for k, s in enumerate(board):
        for i in range(s.n):
            img = V.card_frame(s, i, k == len(board) - 1) if isinstance(s, Card) else V.take_frame(s, i)
            ff.stdin.write(img.tobytes())
            if g in stills:
                img.save(a.stills / f"frame-{g:04d}.png")
            g += 1
    ff.stdin.close()
    ff.wait()
    print(a.out, file=sys.stderr)


if __name__ == "__main__":
    main()
