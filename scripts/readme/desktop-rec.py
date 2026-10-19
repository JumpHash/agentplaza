#!/usr/bin/env python3
"""desktop-rec.py <scene> <out.webp> [dark|light]: records the desktop app on a private Xvfb display as an animated
WebP, for the README (docs/assets/readme/desktop-*.webp).

Every X client this script starts gets DISPLAY=:93 explicitly, never the inherited one, so your own display is never
touched. The app runs against the hub at $APZ_CONNECT (default http://127.0.0.1:7791, a hub with sample data) with a
scratch HOME. Needs Xvfb, xdotool, ffmpeg (libwebp_anim) and Pillow; the app from $APZ_DESKTOP (default
target/release/agentplaza-desktop).
"""
import json
import os
import subprocess
import sys
import tempfile
import threading
import time

from PIL import ImageGrab

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.environ.get("APZ_DESKTOP", os.path.join(ROOT, "target/release/agentplaza-desktop"))
HUB = os.environ.get("APZ_CONNECT", "http://127.0.0.1:7791")
DISP = ":93"
W, H = 1360, 880
FPS = 10


def xenv():
    """A clean environment for X clients on the private display only."""
    env = {k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "DISPLAY", "XAUTHORITY")}
    env["DISPLAY"] = DISP
    return env


def xdo(*args):
    assert DISP == ":93"
    subprocess.run(["xdotool", *args], env=xenv(), check=False, capture_output=True)


def click(x, y, settle=0.25):
    xdo("mousemove", "--sync", str(x), str(y))
    time.sleep(0.25)
    xdo("mousedown", "1")
    time.sleep(0.08)
    xdo("mouseup", "1")
    time.sleep(settle)


def move(x, y):
    xdo("mousemove", "--sync", str(x), str(y))


def key(*keys):
    xdo("key", "--delay", "60", *keys)


def typed(text, delay=45):
    xdo("type", "--delay", str(delay), text)


def scroll(down=True, n=3):
    for _ in range(n):
        xdo("click", "5" if down else "4")
        time.sleep(0.05)


class Recorder:
    def __init__(self):
        self.frames = []
        self.on = False

    def run(self):
        while self.on:
            t = time.time()
            try:
                self.frames.append(ImageGrab.grab(xdisplay=DISP).convert("RGB"))
            except Exception as e:  # the display went away
                print("grab:", e)
                break
            time.sleep(max(0, 1 / FPS - (time.time() - t)))


SCENES = {}


def scene(f):
    SCENES[f.__name__] = f
    return f


# Coordinates are the app's at 1360×880 (sidebar 288 wide), read from deskshot snapshots of the same size.
@scene
def tour(r):
    """Overview with its context bar, then Agents, an agent, Machines."""
    time.sleep(3.0)
    move(860, 560)
    time.sleep(2.0)
    click(70, 240)   # Agents
    time.sleep(2.6)
    click(700, 220)  # the first agent row
    time.sleep(3.0)
    click(70, 272)   # Machines
    time.sleep(5.0)


@scene
def providers(r):
    """Settings › Models: the providers list, an engine, the Add provider dialog."""
    time.sleep(3.0)
    click(700, 650)  # an engine row (Claude Code)
    time.sleep(2.4)
    click(700, 760)  # Codex
    time.sleep(2.4)
    scroll(True, 4)
    time.sleep(1.5)
    scroll(False, 8)
    time.sleep(2.0)


def drag(x0, y0, x1, y1, steps=24, every=0.04):
    xdo("mousemove", "--sync", str(x0), str(y0))
    time.sleep(0.2)
    xdo("mousedown", "1")
    for i in range(1, steps + 1):
        xdo("mousemove", str(int(x0 + (x1 - x0) * i / steps)), str(int(y0 + (y1 - y0) * i / steps)))
        time.sleep(every)
    xdo("mouseup", "1")


@scene
def talk(r):
    """The Overview: a goal typed to the orchestrator, its effort, the context breakdown; then an agent."""
    time.sleep(2.5)
    click(700, 240)
    xdo("search", "--onlyvisible", "--name", "AgentPlaza", "windowfocus", "--sync")
    time.sleep(0.3)
    click(700, 240)
    typed("Ship the pricing page by Friday: split it into tasks for Mira and Kofi", 38)
    time.sleep(0.8)
    click(689, 343)  # effort: High
    time.sleep(1.4)
    click(590, 190)  # the context bar: its breakdown
    time.sleep(3.2)
    click(1250, 760)  # elsewhere: the breakdown closes
    time.sleep(1.0)
    click(64, 240)  # Agents
    time.sleep(2.4)
    click(640, 250)  # an agent
    time.sleep(3.0)


@scene
def machines(r):
    """Machines in 3D: a machine's card, the three styles, an orbit."""
    time.sleep(2.5)
    click(630, 400)  # atlas: its card
    time.sleep(2.6)
    click(1235, 95)  # Blueprint
    time.sleep(2.0)
    click(1300, 95)  # Night
    time.sleep(2.0)
    click(1177, 95)  # Ink
    time.sleep(1.0)
    drag(760, 700, 1000, 640, steps=30)
    time.sleep(2.0)


@scene
def models(r):
    """Settings › Models: providers and engines, one turned on, a favourite."""
    time.sleep(2.5)
    click(700, 620)  # Claude Code
    time.sleep(2.2)
    click(700, 673)  # Codex
    time.sleep(2.2)
    click(700, 360)  # OpenAI: not authenticated
    time.sleep(2.4)
    click(700, 307)  # Anthropic
    time.sleep(2.4)


def main():
    name, out = sys.argv[1], sys.argv[2]
    mode = sys.argv[3] if len(sys.argv) > 3 else "dark"
    screen = {"tour": "Overview", "talk": "Overview", "providers": "Models", "models": "Models", "machines": "Machines"}.get(name, "Overview")
    xvfb = subprocess.Popen(["Xvfb", DISP, "-screen", "0", f"{W}x{H}x24", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.0)
    home = tempfile.mkdtemp(prefix="apz-desktop-rec-")
    env = xenv()
    env.update({
        "HOME": home,
        "APZ_CONNECT": HUB,
        "APZ_SCREEN": screen,
        "APZ_ONBOARD": "off",
        "APZ_RECORDING": "1",
        "APZ_APPEARANCE": json.dumps({"theme": "loom-clay", "mode": mode, "density": "comfortable", "code_size": 13, "chosen": True}),
    })
    app = subprocess.Popen([BIN], env=env, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    rec = Recorder()
    try:
        time.sleep(6.0)  # the window, the engine, the first loads
        xdo("search", "--sync", "--onlyvisible", "--name", "AgentPlaza", "windowmove", "0", "0", "windowsize", str(W), str(H))
        time.sleep(2.0)
        rec.on = True
        t = threading.Thread(target=rec.run)
        t.start()
        SCENES[name](rec)
        rec.on = False
        t.join()
    finally:
        app.terminate()
        try:
            app.wait(5)
        except subprocess.TimeoutExpired:
            app.kill()
        xvfb.terminate()
        xvfb.wait(5)
    print("frames:", len(rec.frames))
    with tempfile.TemporaryDirectory() as tmp:
        for i, im in enumerate(rec.frames):
            im.save(os.path.join(tmp, f"f{i:05d}.png"))
        subprocess.run([
            "ffmpeg", "-loglevel", "error", "-y", "-framerate", str(FPS), "-i", os.path.join(tmp, "f%05d.png"),
            "-vf", "scale=1200:-1:flags=lanczos", "-c:v", "libwebp_anim", "-loop", "0", "-quality", "62",
            "-compression_level", "6", out,
        ], check=True)
        rec.frames[len(rec.frames) // 2].save(out.replace(".webp", "-mid.png"))
        # A contact sheet of eight frames, for checking the whole take at a glance.
        from PIL import Image
        n = len(rec.frames)
        picks = [rec.frames[min(n - 1, int(i * n / 8))].resize((480, 300)) for i in range(8)]
        sheet = Image.new("RGB", (1920, 600))
        for i, im in enumerate(picks):
            sheet.paste(im, ((i % 4) * 480, (i // 4) * 300))
        sheet.save(out.replace(".webp", "-sheet.png"))
    print(out, os.path.getsize(out) // 1024, "KB")


if __name__ == "__main__":
    main()
