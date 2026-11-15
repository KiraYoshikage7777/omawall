#!/usr/bin/env python3
"""Сцену Wallpaper Engine превратить в обычный зацикленный ролик.

Запуск: we-adapt.py <папка сцены> <куда писать .mp4>
Если в сцену вшито готовое видео, оно достаётся как есть. Иначе сцена один раз
записывается: движок рисует её в отдельном невидимом sway, так что на мониторах
ничего не мигает. Нужны sway, wf-recorder, ffmpeg и движок сцен skwd-wall-vk.
"""
import os
import shlex
import struct
import subprocess
import sys
import tempfile

def embedded_video(pkg_path):
    """mp4, вшитый в видео-текстуру scene.pkg Wallpaper Engine (байты), или None."""
    try:
        with open(pkg_path, "rb") as f:
            u32 = lambda: struct.unpack("<I", f.read(4))[0]
            read_str = lambda: f.read(u32())
            read_str()  # PKGV00xx
            items = [(read_str().decode("utf-8", "replace"), u32(), u32()) for _ in range(u32())]
            base = f.tell()
            best = None
            for name, offset, size in items:
                if not name.endswith(".tex"):
                    continue
                f.seek(base + offset)
                head = f.read(min(size, 4096))
                i = head.find(b"ftyp")
                if i > 4 and (best is None or size > best[2]):
                    best = (offset, i - 4, size)
            if best is None:
                return None
            offset, skip, size = best
            f.seek(base + offset + skip)
            return f.read(size - skip)
    except (OSError, struct.error, ValueError):
        return None
def extract_embedded(item, out):
    """Сцена со вшитым mp4 (видео-текстура) — достать ролик как есть."""
    data = embedded_video(os.path.join(item.get("path") or "", "scene.pkg"))
    if not data:
        return False
    raw = out + ".raw"
    with open(raw, "wb") as f:
        f.write(data)
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", raw, "-c", "copy",
                    "-movflags", "+faststart", "-f", "mp4", out + ".tmp"],
                   capture_output=True, timeout=120)
    os.remove(raw)
    if not os.path.exists(out + ".tmp"):
        return False
    os.replace(out + ".tmp", out)
    return True
RECORD_SECONDS = 30  # длина петли
LOOP_FADE = 2        # конец петли плавно переходит в начало

# Запись идёт в отдельном невидимом sway (headless): ни Hyprland, ни демон skwd
# о нём не знают, поэтому на твоих мониторах при записи ничего не мигает.
RECORD_JOB = r"""#!/bin/bash
# $1 — папка сцены, $2 — куда писать, $3 — секунд записи. Запускается внутри sway.
sleep 3600 | skwd-wall-vk HEADLESS-1 "$1" --scene "$1" --standalone --layer background -o mute=yes &
sleep 5  # сцене дать загрузиться и разогнаться
timeout -s INT "$3" wf-recorder -D -o HEADLESS-1 -c libx264 -r 30 -p preset=veryfast -p crf=18 -f "$2"
pkill -P $$
swaymsg exit
"""


def media_seconds(path):
    probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration",
                            "-of", "csv=p=0", path], capture_output=True, text=True)
    try:
        return float(probe.stdout.strip())
    except ValueError:
        return 0.0


def loop_encode(raw, out):
    """Склеить конец записи с началом плавным переходом, чтобы петля не прыгала."""
    probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration",
                            "-of", "csv=p=0", raw], capture_output=True, text=True)
    try:
        total = float(probe.stdout.strip())
    except ValueError:
        return False
    if total < 3 * LOOP_FADE:
        return False
    # fps: запись идёт с переменной частотой кадров, а xfade нужна постоянная.
    graph = (f"[0:v]fps=30,split[a][b];"
             f"[a]trim=start={LOOP_FADE},setpts=PTS-STARTPTS[main];"
             f"[b]trim=end={LOOP_FADE},setpts=PTS-STARTPTS[head];"
             f"[main][head]xfade=transition=fade:duration={LOOP_FADE}:offset={total - 2 * LOOP_FADE:.3f}[v]")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", raw, "-filter_complex", graph, "-map", "[v]",
                    "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-pix_fmt", "yuv420p",
                    "-movflags", "+faststart", "-f", "mp4", out + ".tmp"],
                   capture_output=True, timeout=600)
    if not os.path.exists(out + ".tmp"):
        return False
    os.replace(out + ".tmp", out)
    return True


def record_scene(item, out):
    """Один раз записать сцену в ролик: движок рисует её в отдельном headless sway."""
    folder = item.get("path") or ""
    raw = out + ".rec.mp4"
    if os.path.exists(raw):
        os.remove(raw)
    runtime = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")
    with tempfile.TemporaryDirectory(prefix="skwd-rec-", dir=runtime) as work:
        job = os.path.join(work, "job.sh")
        with open(job, "w") as f:
            f.write(RECORD_JOB)
        os.chmod(job, 0o755)
        config = os.path.join(work, "sway.conf")
        with open(config, "w") as f:
            f.write("output HEADLESS-1 resolution 1920x1080\n"
                    f"exec {shlex.quote(job)} {shlex.quote(folder)} {shlex.quote(raw)} "
                    f"{RECORD_SECONDS + LOOP_FADE}\n")
        env = {k: v for k, v in os.environ.items()
               if k not in ("WAYLAND_DISPLAY", "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK")}
        env.update({"WLR_BACKENDS": "headless", "WLR_LIBINPUT_NO_DEVICES": "1"})
        try:
            subprocess.run(["sway", "--unsupported-gpu", "-c", config], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           timeout=RECORD_SECONDS + 90)
        except subprocess.TimeoutExpired:
            pass
    # Брак (короткая запись) не принимаем: сцена останется сценой.
    ok = (os.path.exists(raw) and media_seconds(raw) >= 0.8 * (RECORD_SECONDS + LOOP_FADE)
          and loop_encode(raw, out) and media_seconds(out) >= 0.8 * RECORD_SECONDS)
    if os.path.exists(raw):
        os.remove(raw)
    if not ok and os.path.exists(out):
        os.remove(out)
    return ok


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: we-adapt.py <scene dir> <out.mp4>")
    scene, target = {"path": sys.argv[1]}, sys.argv[2]
    os.makedirs(os.path.dirname(target), exist_ok=True)
    sys.exit(0 if extract_embedded(scene, target) or record_scene(scene, target) else 1)
