#!/usr/bin/env python3
"""Generate the built-in Shimeji-ee-format pet packs (pixel sprites, no deps).

Writes src-tauri/pets/<name>/{conf/{actions,behaviors}.xml, img/shime*.png}.
Canvases are 72×72 — native 1× at the default 72 px pet size (2 device px per
art pixel on Retina), so the sprite is crisp without uneven scaling. Art faces
LEFT natively; the engine mirrors for facing-right.

Rerun after editing a drawing function:  python3 scripts/make-pets.py
"""

import struct
import zlib
from pathlib import Path

SIZE = 72
ROOT = Path(__file__).resolve().parent.parent / "src-tauri" / "pets"


def png_bytes(pixels):
    raw = b"".join(
        b"\x00" + b"".join(struct.pack("4B", *px) for px in row) for row in pixels
    )

    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def canvas():
    return [[(0, 0, 0, 0)] * SIZE for _ in range(SIZE)]


def px(c, x, y, color):
    if 0 <= x < SIZE and 0 <= y < SIZE:
        c[y][x] = color


def rect(c, x0, y0, x1, y1, color):
    """Inclusive-corner rectangle."""
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            px(c, x, y, color)


def hline(c, x0, x1, y, color):
    rect(c, x0, y, x1, y, color)


def vline(c, x, y0, y1, color):
    rect(c, x, y0, x, y1, color)


def outline_row(c, x0, x1, y, color):
    """Dark seam under a body mass."""
    hline(c, x0, x1, y, color)


# ── palette ──────────────────────────────────────────────────────────
ORANGE = (243, 150, 64, 255)
ORANGE_M = (222, 122, 42, 255)   # shading
ORANGE_D = (178, 92, 30, 255)    # deep shading / legs
CREAM = (255, 228, 185, 255)
INK = (46, 36, 32, 255)
WHITE = (252, 248, 240, 255)
PINK = (242, 132, 132, 255)
PINK_D = (214, 96, 108, 255)
TEAL = (92, 202, 192, 255)
TEAL_L = (168, 232, 224, 255)
TEAL_M = (62, 164, 158, 255)
TEAL_D = (38, 122, 122, 255)
RED = (226, 92, 92, 255)

# ── cat drawing (faces LEFT) ─────────────────────────────────────────
# Chibi proportions: big head (dominates), small tucked body, stubby legs.
# Ground line: y=70 (feet). Anchor: (36, 72) = bottom center.


def cat_head(c, ox=0, oy=0, eye="open", ears="up"):
    """Big head block x 4..45, y 4..41 (before offset)."""
    hx, hy = 4 + ox, 4 + oy
    # ears (behind head)
    if ears == "up":
        rect(c, hx + 2, hy - 6, hx + 12, hy + 3, ORANGE)
        rect(c, hx + 4, hy - 4, hx + 8, hy + 0, PINK)
        rect(c, hx + 28, hy - 6, hx + 38, hy + 3, ORANGE)
        rect(c, hx + 30, hy - 4, hx + 34, hy + 0, PINK)
    elif ears == "back":  # startled / falling
        rect(c, hx + 6, hy - 5, hx + 16, hy + 1, ORANGE)
        rect(c, hx + 30, hy - 3, hx + 40, hy + 2, ORANGE)
    # head
    rect(c, hx, hy, hx + 40, hy + 30, ORANGE)
    rect(c, hx, hy + 30, hx + 40, hy + 36, ORANGE_M)       # jaw shade
    # forehead stripes
    hline(c, hx + 14, hx + 17, hy + 2, ORANGE_M)
    hline(c, hx + 21, hx + 24, hy + 2, ORANGE_M)
    hline(c, hx + 28, hx + 31, hy + 2, ORANGE_M)
    # muzzle
    rect(c, hx - 2, hy + 20, hx + 14, hy + 32, CREAM)
    rect(c, hx + 14, hy + 26, hx + 18, hy + 32, CREAM)
    px(c, hx - 3, hy + 24, PINK)                            # nose
    px(c, hx - 4, hy + 24, PINK)
    # whisker dots
    px(c, hx + 2, hy + 28, ORANGE_D)
    px(c, hx + 6, hy + 29, ORANGE_D)
    px(c, hx + 10, hy + 28, ORANGE_D)
    # eye (big, with highlight)
    ex, ey = hx + 20, hy + 10
    if eye == "open":
        rect(c, ex, ey, ex + 9, ey + 10, WHITE)
        rect(c, ex + 1, ey + 1, ex + 7, ey + 9, INK)
        rect(c, ex + 2, ey + 2, ex + 4, ey + 4, WHITE)      # highlight
        px(c, ex + 6, ey + 7, WHITE)
        rect(c, ex + 10, ey + 3, ex + 11, ey + 9, INK)      # far lid edge
    elif eye == "happy":                                    # ^ for joy
        hline(c, ex, ex + 3, ey + 4, INK)
        px(c, ex + 4, ey + 3, INK)
        px(c, ex + 5, ey + 4, INK)
        hline(c, ex + 6, ex + 9, ey + 5, INK)
    elif eye == "blink":
        hline(c, ex, ex + 10, ey + 5, INK)
    elif eye == "dizzy":
        rect(c, ex, ey, ex + 9, ey + 9, WHITE)
        hline(c, ex, ex + 9, ey + 4, RED)
        vline(c, ex + 4, ey, ey + 9, RED)
    # mouth
    if eye == "happy":
        rect(c, hx + 1, hy + 27, hx + 7, hy + 29, PINK_D)   # open smile
    else:
        px(c, hx + 1, hy + 30, INK)
        px(c, hx + 2, hy + 31, INK)


def cat_body(c, ox=0, oy=0):
    """Small torso tucked behind the head: x 32..64, y 38..62."""
    bx, by = 32 + ox, 38 + oy
    rect(c, bx, by, bx + 31, by + 20, ORANGE)
    rect(c, bx, by + 20, bx + 31, by + 24, ORANGE_M)
    # cream chest on the left (facing side)
    rect(c, bx - 2, by + 6, bx + 8, by + 20, CREAM)
    # back stripes
    hline(c, bx + 16, bx + 24, by + 2, ORANGE_M)
    hline(c, bx + 14, bx + 22, by + 8, ORANGE_M)
    hline(c, bx + 12, bx + 20, by + 14, ORANGE_M)


def cat_tail(c, ox=0, oy=0, pose="up"):
    """Tail from the rump (right side), curled up."""
    tx, ty = 60 + ox, 38 + oy
    if pose == "up":
        rect(c, tx + 6, ty + 4, tx + 10, ty + 20, ORANGE_M)
        rect(c, tx + 4, ty - 6, tx + 12, ty + 6, ORANGE)
        rect(c, tx + 5, ty - 12, tx + 11, ty - 4, CREAM)    # white tip
    elif pose == "sway":  # idle frame B: tip flicked
        rect(c, tx + 6, ty + 4, tx + 10, ty + 20, ORANGE_M)
        rect(c, tx + 0, ty + 0, tx + 10, ty + 8, ORANGE)
        rect(c, tx - 6, ty - 4, tx + 2, ty + 3, CREAM)
    elif pose == "curl":  # sitting: wrapped along the ground
        hline(c, tx - 30, tx + 8, ty + 22, ORANGE_M)
        rect(c, tx - 38, ty + 14, tx - 30, ty + 24, ORANGE)
        rect(c, tx - 44, ty + 10, tx - 36, ty + 20, CREAM)
    elif pose == "out":  # falling / surprised: straight back
        rect(c, tx - 2, ty + 4, tx + 14, ty + 10, ORANGE_M)
        rect(c, tx + 12, ty - 2, tx + 20, ty + 8, ORANGE)
        rect(c, tx + 18, ty - 8, tx + 24, ty + 0, CREAM)


def cat_legs(c, frame, oy=0):
    """4-frame walk cycle; frame 0 = stand."""
    gy = 70 + oy  # ground line
    if frame == 0:  # stand
        rect(c, 36, 58, 44, gy - 1, ORANGE_M)
        rect(c, 54, 58, 62, gy - 1, ORANGE_M)
        hline(c, 35, 45, gy, INK)
        hline(c, 53, 63, gy, INK)
    elif frame == 1:  # walk: front leg forward, back leg back
        rect(c, 28, 58, 36, gy - 1, ORANGE_M)
        rect(c, 58, 58, 64, gy - 1, ORANGE_D)
        hline(c, 27, 37, gy, INK)
        hline(c, 57, 65, gy, INK)
    elif frame == 2:  # passing: legs together
        rect(c, 38, 58, 46, gy - 1, ORANGE_M)
        rect(c, 50, 58, 58, gy - 1, ORANGE_M)
        hline(c, 37, 47, gy, INK)
        hline(c, 49, 59, gy, INK)
    elif frame == 3:  # front leg back, back leg forward
        rect(c, 40, 58, 48, gy - 1, ORANGE_D)
        rect(c, 46, 58, 52, gy - 1, ORANGE_M)
        hline(c, 39, 49, gy, INK)
        hline(c, 45, 53, gy, INK)


def draw_cat(frame):
    """frame ids: 0 stand-a, 1 stand-b, 2..5 walk, 6 fall, 7 jump, 8 trip-a,
    9 trip-b, 10 sit, 11 drag."""
    c = canvas()
    if frame in (0, 1):
        bob = -1 if frame == 1 else 0
        cat_tail(c, 0, bob, pose="sway" if frame == 1 else "up")
        cat_body(c, 0, bob)
        cat_legs(c, 0, bob)
        cat_head(c, 0, bob, eye="blink" if frame == 1 else "open")
    elif frame in (2, 3, 4, 5):
        leg = frame - 2
        bob = -2 if leg in (1, 3) else 0
        cat_tail(c, 0, bob, pose="up")
        cat_body(c, 0, bob)
        cat_legs(c, 1 if leg in (0, 3) else 2 if leg == 1 else 3, bob)
        cat_head(c, -1 if leg in (0, 2) else 1, bob)
    elif frame == 6:  # fall
        cat_tail(c, 0, 0, pose="out")
        cat_body(c, 0, -2)
        # splayed legs
        rect(c, 32, 50, 38, 60, ORANGE_D)
        rect(c, 58, 50, 64, 60, ORANGE_D)
        rect(c, 26, 26, 32, 38, ORANGE_D)
        cat_head(c, 0, -4, eye="dizzy", ears="back")
    elif frame == 7:  # jump / celebrate
        cat_tail(c, 2, -8, pose="up")
        cat_body(c, 2, -10)
        rect(c, 34, 40, 40, 48, ORANGE_D)   # tucked legs
        rect(c, 56, 40, 62, 48, ORANGE_D)
        cat_head(c, 2, -14, eye="happy")
        rect(c, 24, 36, 30, 46, ORANGE_M)   # front paw raised
    elif frame == 8:  # trip-a: stumbling forward
        cat_tail(c, 2, 6, pose="out")
        cat_body(c, 4, 4)
        cat_legs(c, 3, 4)
        cat_head(c, -8, 8, eye="dizzy")
    elif frame == 9:  # trip-b: faceplant
        rect(c, 28, 50, 66, 66, ORANGE)
        outline_row(c, 28, 66, 67, ORANGE_D)
        rect(c, 12, 16, 52, 46, ORANGE)     # head sideways-low
        rect(c, 6, 8, 16, 20, ORANGE)       # ear
        rect(c, 9, 11, 13, 16, PINK)
        rect(c, 20, 30, 32, 42, WHITE)      # dizzy eye
        hline(c, 20, 32, 36, RED)
        vline(c, 26, 30, 42, RED)
        rect(c, 2, 34, 18, 44, CREAM)
        rect(c, 62, 36, 70, 46, ORANGE_D)   # tail up in surprise
    elif frame == 10:  # sit
        cat_tail(c, 0, 0, pose="curl")
        # haunch
        rect(c, 36, 36, 64, 66, ORANGE)
        rect(c, 36, 62, 64, 66, ORANGE_M)
        hline(c, 38, 60, 39, ORANGE_M)
        # chest + front paws
        rect(c, 22, 44, 40, 66, ORANGE)
        rect(c, 20, 60, 32, 66, CREAM)
        hline(c, 19, 33, 67, INK)
        hline(c, 35, 65, 67, INK)
        cat_head(c, 0, 4, eye="open")
    elif frame == 11:  # dragged: held up, limbs dangling
        cat_tail(c, 4, -2, pose="out")
        cat_body(c, 2, -4)
        rect(c, 34, 52, 40, 64, ORANGE_M)   # dangling legs
        rect(c, 56, 52, 62, 64, ORANGE_M)
        hline(c, 33, 41, 65, INK)
        hline(c, 55, 63, 65, INK)
        cat_head(c, 2, -6, eye="open", ears="back")
        rect(c, 24, 40, 30, 50, ORANGE_M)   # splayed front arm
    return c


# ── slime drawing (faces LEFT) ───────────────────────────────────────

def slime_body(c, x0, x1, top, bottom, gloss=True):
    rect(c, x0, top, x1, bottom, TEAL)
    rect(c, x0, bottom - 3, x1, bottom, TEAL_M)
    hline(c, x0, x1, bottom + 1, TEAL_D)          # ground shadow seam
    if gloss:
        rect(c, x0 + 6, top + 3, x0 + 16, top + 8, TEAL_L)
        px(c, x0 + 17, top + 9, TEAL_L)


def slime_face(c, x, y, eye="open"):
    if eye == "open":
        rect(c, x, y, x + 4, y + 5, INK)
        rect(c, x + 10, y, x + 14, y + 5, INK)
        px(c, x + 1, y + 1, WHITE)
        px(c, x + 11, y + 1, WHITE)
        rect(c, x + 4, y + 8, x + 10, y + 9, TEAL_D)  # mouth
    elif eye == "happy":
        hline(c, x, x + 2, y + 2, INK)
        px(c, x + 3, y + 1, INK)
        px(c, x + 4, y + 2, INK)
        hline(c, x + 10, x + 12, y + 2, INK)
        px(c, x + 8, y + 2, INK)
        rect(c, x + 4, y + 7, x + 10, y + 9, TEAL_D)
    elif eye == "dizzy":
        rect(c, x, y, x + 4, y + 5, WHITE)
        hline(c, x, x + 4, y + 2, RED)
        vline(c, x + 2, y, y + 5, RED)
        rect(c, x + 10, y, x + 14, y + 5, WHITE)
        hline(c, x + 10, x + 14, y + 2, RED)
        vline(c, x + 12, y, y + 5, RED)


def draw_slime(frame):
    """0 idle-a, 1 idle-b (squash), 2 idle-c (stretch), 3 hop-squash,
    4 hop-stretch, 5 fall, 6 jump, 7 trip, 8 sit/pancake, 9 drag."""
    c = canvas()
    gy = 70
    if frame == 0:
        slime_body(c, 12, 58, 28, gy - 1)
        slime_face(c, 22, 42)
    elif frame == 1:  # squash
        slime_body(c, 8, 62, 38, gy - 1)
        slime_face(c, 22, 48)
    elif frame == 2:  # stretch
        slime_body(c, 16, 54, 18, gy - 1)
        slime_face(c, 24, 34)
    elif frame == 3:  # hop: squashed launch
        slime_body(c, 10, 60, 40, gy - 1)
        slime_face(c, 22, 50, eye="happy")
    elif frame == 4:  # hop: stretched in air
        slime_body(c, 18, 52, 12, gy - 3)
        slime_face(c, 26, 26, eye="happy")
    elif frame == 5:  # fall: teardrop
        rect(c, 28, 10, 42, 22, TEAL)
        rect(c, 22, 20, 48, 32, TEAL)
        slime_body(c, 14, 56, 30, gy - 1, gloss=False)
        slime_face(c, 24, 44, eye="dizzy")
    elif frame == 6:  # jump / celebrate
        slime_body(c, 20, 50, 6, gy - 9)
        slime_face(c, 26, 18, eye="happy")
    elif frame == 7:  # trip: flattened + dizzy
        slime_body(c, 6, 64, 48, gy - 1, gloss=False)
        slime_face(c, 22, 52, eye="dizzy")
    elif frame == 8:  # sit: relaxed pancake
        slime_body(c, 10, 60, 46, gy - 1)
        slime_face(c, 22, 52, eye="open")
        # resting puddle rim
        hline(c, 8, 62, gy, TEAL_D)
    elif frame == 9:  # dragged: drooping
        rect(c, 30, 8, 42, 20, TEAL)
        slime_body(c, 16, 56, 18, gy - 1, gloss=False)
        slime_face(c, 24, 30, eye="dizzy")
    return c


# ── pack XML ─────────────────────────────────────────────────────────

ANCHOR = 'ImageAnchor="36,72"'


def cat_frames():
    return [draw_cat(i) for i in range(12)]


def slime_frames():
    return [draw_slime(i) for i in range(10)]


def walk_poses(frames, velocity, duration):
    return "\n".join(
        f'        <Pose Image="/shime{i + 1}.png" {ANCHOR} Velocity="{velocity},0" Duration="{duration}" />'
        for i in frames
    )


def write_pack(name, frames, walk_frames, stand_frames, walk_velocity=1, walk_duration=8):
    out = ROOT / name
    (out / "conf").mkdir(parents=True, exist_ok=True)
    (out / "img").mkdir(parents=True, exist_ok=True)
    for i, pixels in enumerate(frames, start=1):
        (out / "img" / f"shime{i}.png").write_bytes(png_bytes(pixels))

    stand = walk_poses(stand_frames, 0, 60)
    walk = walk_poses(walk_frames, -walk_velocity, walk_duration)
    run = walk_poses(walk_frames, -walk_velocity * 3, max(2, walk_duration // 3))
    actions = f"""<?xml version="1.0" encoding="UTF-8"?>
<Mascot xmlns="http://www.group-finity.com/Mascot">
  <ActionList>
    <Action Name="Stand" Type="Stay" BorderType="Floor">
      <Animation>
{stand}
      </Animation>
    </Action>
    <Action Name="Walk" Type="Move" BorderType="Floor">
      <Animation>
{walk}
      </Animation>
    </Action>
    <Action Name="Run" Type="Move" BorderType="Floor">
      <Animation>
{run}
      </Animation>
    </Action>
    <Action Name="Falling" Type="Embedded" Class="com.group_finity.mascot.action.Fall" Gravity="2" RegistanceX="0.05" RegistanceY="0.1">
      <Animation><Pose Image="/shime{FALL_FRAME[name]}.png" {ANCHOR} Velocity="0,0" Duration="250" /></Animation>
    </Action>
    <Action Name="Jumping" Type="Embedded" Class="com.group_finity.mascot.action.Jump" VelocityParam="16">
      <Animation><Pose Image="/shime{JUMP_FRAME[name]}.png" {ANCHOR} Velocity="0,0" Duration="250" /></Animation>
    </Action>
    <Action Name="Bouncing" Type="Animate" BorderType="Floor">
      <Animation>
        <Pose Image="/shime{JUMP_FRAME[name]}.png" {ANCHOR} Velocity="0,0" Duration="8" />
        <Pose Image="/shime1.png" {ANCHOR} Velocity="0,0" Duration="8" />
      </Animation>
    </Action>
    <Action Name="Tripping" Type="Animate" BorderType="Floor">
      <Animation>
        <Pose Image="/shime{TRIP_FRAMES[name][0]}.png" {ANCHOR} Velocity="-1,0" Duration="10" />
        <Pose Image="/shime{TRIP_FRAMES[name][1]}.png" {ANCHOR} Velocity="0,0" Duration="6" />
      </Animation>
    </Action>
    <Action Name="SitDown" Type="Stay" BorderType="Floor">
      <Animation><Pose Image="/shime{SIT_FRAME[name]}.png" {ANCHOR} Velocity="0,0" Duration="120" /></Animation>
    </Action>
    <Action Name="Dragged" Type="Embedded" Class="com.group_finity.mascot.action.Dragged">
      <Animation><Pose Image="/shime{DRAG_FRAME[name]}.png" {ANCHOR} Velocity="0,0" Duration="250" /></Animation>
    </Action>
  </ActionList>
</Mascot>
"""
    (out / "conf" / "actions.xml").write_text(actions, encoding="utf-8")
    behaviors = f"""<?xml version="1.0" encoding="UTF-8"?>
<Mascot xmlns="http://www.group-finity.com/Mascot">
  <BehaviorList>
    <Behavior Name="Fall" Frequency="0" />
    <Behavior Name="Dragged" Frequency="0" />
    <Behavior Name="Thrown" Frequency="0" />
    <Behavior Name="Stand" Frequency="200" />
    <Behavior Name="Walk" Frequency="100">
      <NextBehavior Add="false">
        <BehaviorReference Name="Stand" Frequency="100" />
      </NextBehavior>
    </Behavior>
    <Behavior Name="SitDown" Frequency="40" />
  </BehaviorList>
</Mascot>
"""
    (out / "conf" / "behaviors.xml").write_text(behaviors, encoding="utf-8")
    print(f"wrote {out}")


# Frame index maps (1-based shimeN.png).
FALL_FRAME = {"agent-cat": 7, "byte-slime": 6}
JUMP_FRAME = {"agent-cat": 8, "byte-slime": 7}
SIT_FRAME = {"agent-cat": 11, "byte-slime": 9}
DRAG_FRAME = {"agent-cat": 12, "byte-slime": 10}
TRIP_FRAMES = {"agent-cat": (9, 10), "byte-slime": (8, 1)}


if __name__ == "__main__":
    write_pack("agent-cat", cat_frames(), walk_frames=[3, 4, 5, 6], stand_frames=[1, 2])
    write_pack("byte-slime", slime_frames(), walk_frames=[4, 5], stand_frames=[1, 2, 3])
