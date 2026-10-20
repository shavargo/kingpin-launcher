"""Re-cut rain-noir chrome: transparent logo letters, blank plates, labeled JOIN."""
from __future__ import annotations

from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "assets" / "chrome"


def scrub_plate(src: Image.Image, box: tuple[int, int, int, int]) -> Image.Image:
    """Crop a plate and obliterate the centered label with tiled metal noise."""
    plate = src.crop(box).convert("RGBA")
    arr = np.array(plate)
    pw, ph = plate.size
    # Sample metal from the four border strips (outside the letter box).
    band = max(5, ph // 6)
    side = max(8, pw // 12)
    samples = [
        arr[:band, :],
        arr[-band:, :],
        arr[:, :side],
        arr[:, -side:],
    ]
    pool = np.concatenate([s.reshape(-1, 4) for s in samples], axis=0)
    # Rebuild every inner pixel from a random neighbor in the metal pool.
    rng = np.random.default_rng(42)
    out = arr.copy()
    ix0, iy0 = side, band
    ix1, iy1 = pw - side, ph - band
    n = pool.shape[0]
    for y in range(iy0, iy1):
        picks = rng.integers(0, n, size=(ix1 - ix0,))
        out[y, ix0:ix1] = pool[picks]
    # Soft blur the scrubbed core so it reads as metal, keep riveted rim sharp.
    core = Image.fromarray(out, "RGBA")
    blurred = core.filter(ImageFilter.GaussianBlur(0.8))
    mask = Image.new("L", (pw, ph), 0)
    ImageDraw.Draw(mask).rectangle((ix0, iy0, ix1 - 1, iy1 - 1), fill=255)
    mask = mask.filter(ImageFilter.GaussianBlur(1.2))
    core = Image.composite(blurred, Image.fromarray(arr, "RGBA"), mask)
    # Kill any leftover bright letter pixels in the core by luminance clamp.
    c = np.array(core)
    lum = c[:, :, 0].astype(np.int16) + c[:, :, 1] + c[:, :, 2]
    hot = (lum > 280) & (np.array(mask) > 40)
    if hot.any():
        c[hot, :3] = (c[hot, :3].astype(np.int16) * 0.35).astype(np.uint8)
    c[:, :, 3] = 255
    return Image.fromarray(c, "RGBA")


def main() -> None:
    src = Image.open(OUT / "mockup_full.jpg").convert("RGBA")
    w, h = src.size
    print("mockup", src.size)

    # Full logo band — leave room for drips + TM
    logo = src.crop((290, 8, 740, 108)).convert("RGBA")
    arr = np.array(logo)
    r = arr[:, :, 0].astype(np.int16)
    g = arr[:, :, 1].astype(np.int16)
    b = arr[:, :, 2].astype(np.int16)
    lum = r + g + b
    # Distressed metal letters: warm OR bright cream; reject cyan rain
    warm = ((r >= g - 8) & (r > 45) & (lum > 100)) | ((lum > 220) & (b < r + 30))
    not_rain = ~((b > r + 18) & (b > g + 8) & (b > 50))
    mask = warm & not_rain
    mimg = Image.fromarray((mask.astype(np.uint8) * 255), "L")
    mimg = mimg.filter(ImageFilter.MaxFilter(5))
    mimg = mimg.filter(ImageFilter.MinFilter(3))
    mimg = mimg.filter(ImageFilter.GaussianBlur(0.8))
    marr = np.array(mimg)
    out = arr.copy()
    out[:, :, 3] = marr
    out[marr < 12] = [0, 0, 0, 0]
    Image.fromarray(out, "RGBA").save(OUT / "logo.png")
    print("logo", out.shape, "opaque", int((marr > 12).sum()))

    scrub_plate(src, (48, 138, 268, 188)).save(OUT / "btn_idle.png")
    scrub_plate(src, (42, 252, 274, 305)).save(OUT / "btn_selected.png")
    print("plates ok")

    # JOIN with baked label — keep red plate + JOIN text
    join = src.crop((760, 470, 1005, 555)).convert("RGBA")
    j = np.array(join)
    lum = j[:, :, 0].astype(np.int16) + j[:, :, 1] + j[:, :, 2]
    keep = lum > 35
    jm = Image.fromarray((keep.astype(np.uint8) * 255), "L").filter(
        ImageFilter.MaxFilter(3)
    )
    j[:, :, 3] = np.array(jm)
    j[np.array(jm) < 8] = [0, 0, 0, 0]
    Image.fromarray(j, "RGBA").save(OUT / "join_labeled.png")
    Image.fromarray(j, "RGBA").save(OUT / "join.png")
    print("join", j.shape)
    print("done")


if __name__ == "__main__":
    main()
