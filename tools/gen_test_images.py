#!/usr/bin/env python3
"""Genera JPGs de prueba (pequeños, medianos y grandes) con EXIF para medir PicaRust.

Uso: python3 tools/gen_test_images.py [carpeta_destino]   (por defecto: fotos_prueba/)
Requiere Pillow (pip install pillow).
"""
import math
import random
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont
from PIL.TiffImagePlugin import IFDRational as R

OUT = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent / "fotos_prueba")

# (nombre, ancho, alto, calidad, orientación EXIF)
SIZES = [
    ("01_pequena_320x240", 320, 240, 85, 1),
    ("02_pequena_640x480", 640, 480, 85, 1),
    ("03_pequena_1024x768", 1024, 768, 88, 1),
    ("04_mediana_1920x1080", 1920, 1080, 90, 1),
    ("05_mediana_2560x1440", 2560, 1440, 90, 1),
    ("06_mediana_3000x2000_6MP", 3000, 2000, 90, 1),
    ("07_mediana_4000x3000_12MP", 4000, 3000, 92, 1),
    ("08_retrato_exif_rotada_4000x3000", 4000, 3000, 90, 6),  # debe verse en vertical
    ("09_grande_6000x4000_24MP", 6000, 4000, 92, 1),
]

CAMERAS = [("Canon", "EOS R6"), ("Nikon", "Z 6II"), ("Sony", "ILCE-7M4"), ("Fujifilm", "X-T5")]


def font(size):
    for f in ("DejaVuSans-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"):
        try:
            return ImageFont.truetype(f, size)
        except OSError:
            pass
    return ImageFont.load_default()


def make(name, w, h, seed):
    rnd = random.Random(seed)
    # Degradado de fondo (rápido: se genera pequeño y se escala).
    c1 = [rnd.randint(0, 255) for _ in range(3)]
    c2 = [rnd.randint(0, 255) for _ in range(3)]
    grad = Image.new("RGB", (256, 2))
    for x in range(256):
        t = x / 255
        col = tuple(int(c1[i] * (1 - t) + c2[i] * t) for i in range(3))
        grad.putpixel((x, 0), col)
        grad.putpixel((x, 1), col)
    img = grad.resize((w, h), Image.BILINEAR)

    d = ImageDraw.Draw(img, "RGBA")
    # Círculos y rectángulos para tener bordes con los que juzgar el zoom.
    for _ in range(60):
        r = rnd.randint(min(w, h) // 40, min(w, h) // 6)
        x, y = rnd.randint(0, w), rnd.randint(0, h)
        col = tuple(rnd.randint(0, 255) for _ in range(3)) + (rnd.randint(60, 180),)
        if rnd.random() < 0.5:
            d.ellipse((x - r, y - r, x + r, y + r), fill=col)
        else:
            d.rectangle((x - r, y - r // 2, x + r, y + r // 2), fill=col)
    # Rejilla fina para comprobar la nitidez a 100 % y más.
    step = max(16, min(w, h) // 30)
    for x in range(0, w, step):
        d.line((x, 0, x, h), fill=(255, 255, 255, 40), width=1)
    for y in range(0, h, step):
        d.line((0, y, w, y), fill=(255, 255, 255, 40), width=1)
    # Espiral de detalle en el centro.
    cx, cy = w / 2, h / 2
    pts = [(cx + a * math.cos(a / 8) * min(w, h) / 900, cy + a * math.sin(a / 8) * min(w, h) / 900)
           for a in range(0, 400)]
    d.line(pts, fill=(255, 255, 255, 220), width=max(1, w // 1500))
    # Texto grande y texto diminuto (sólo legible con zoom).
    d.text((w * 0.04, h * 0.05), name, font=font(max(14, w // 28)), fill=(255, 255, 255, 235))
    d.text((cx + 4, cy + 4), f"{w}x{h} - detalle a 100%", font=font(max(8, w // 400)), fill=(255, 255, 0, 255))
    # Ruido para que el JPEG pese como una foto real.
    noise = Image.effect_noise((w, h), 28).convert("RGB")
    return Image.blend(img, noise, 0.12)


def exif_for(i, w, h, orientation):
    make_, model = CAMERAS[i % len(CAMERAS)]
    ex = Image.Exif()
    ex[0x010F] = make_                     # Make
    ex[0x0110] = model                     # Model
    ex[0x0131] = "PicaRust test generator" # Software
    ex[0x013B] = "PicaRust"                # Artist
    ex[0x0112] = orientation               # Orientation
    ex[0x0132] = f"2026:11:0{1 + i % 9} 1{i % 10}:2{i % 6}:00"
    sub = ex.get_ifd(0x8769)
    sub[0x9003] = f"2026:11:0{1 + i % 9} 1{i % 10}:2{i % 6}:00"  # DateTimeOriginal
    sub[0x829A] = R(1, [60, 125, 250, 500, 1000][i % 5])           # ExposureTime
    sub[0x829D] = R([18, 28, 40, 56, 80][i % 5], 10)               # FNumber
    sub[0x8827] = [100, 200, 400, 800, 1600][i % 5]               # ISO
    sub[0x920A] = R([24, 35, 50, 85, 135][i % 5], 1)               # FocalLength
    sub[0xA434] = "RF 24-105mm F4 L IS USM"                       # LensModel
    sub[0x9209] = 16                                              # Flash: no disparado
    sub[0xA002], sub[0xA003] = w, h
    # GPS (Valencia) en las imágenes pares.
    if i % 2 == 0:
        gps = ex.get_ifd(0x8825)
        gps[1], gps[2] = "N", (R(39), R(28), R(2))
        gps[3], gps[4] = "W", (R(0), R(22), R(35))
    return ex


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for i, (name, w, h, q, orient) in enumerate(SIZES):
        img = make(name, w, h, i)
        path = OUT / f"{name}.jpg"
        img.save(path, "JPEG", quality=q, exif=exif_for(i, w, h, orient), optimize=False)
        print(f"{path.name:45s} {path.stat().st_size / 1024:9.0f} KB")
    print(f"\nListo: {OUT}")


if __name__ == "__main__":
    main()
