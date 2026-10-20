"""Extract launcher chrome from the rain-noir mockup (1024x576)."""
from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[1]
MOCKUP = Path(
    r"C:\Users\calho\.cursor\projects\c-bws-development-kingpin-launcher\assets"
    r"\c__Users_calho_AppData_Roaming_Cursor_User_workspaceStorage_"
    r"0f42ff5168664392e00bef6051165c38_images_image-760d32f4-6efe-47cc-9db2-f6fe32e7e7f8.png"
)
OUT = ROOT / "assets" / "chrome"
OUT.mkdir(parents=True, exist_ok=True)


def blank_plate(src: Image.Image, box: tuple[int, int, int, int], out: Path) -> None:
    """Crop a plate and scrub centered label by cloning metal from top/bottom bands."""
    plate = src.crop(box).convert("RGBA")
    w, h = plate.size
    top = plate.crop((0, 0, w, max(6, h // 5)))
    bot = plate.crop((0, h - max(6, h // 5), w, h))
    fill = Image.new("RGBA", (w, h))
    for y in range(h):
        row_src = top if (y / max(1, h - 1)) < 0.5 else bot
        yy = min(row_src.height - 1, int((y / h) * (row_src.height - 1)))
        strip = row_src.crop((0, yy, w, yy + 1)).resize((w, 1))
        fill.paste(strip, (0, y))
    pad_x, pad_y = int(w * 0.08), int(h * 0.22)
    inner = (pad_x, pad_y, w - pad_x, h - pad_y)
    fill_blur = fill.filter(ImageFilter.GaussianBlur(1.2))
    mask = Image.new("L", (w, h), 0)
    ImageDraw.Draw(mask).rectangle(inner, fill=255)
    plate = Image.composite(fill_blur, plate, mask)
    plate.save(out)


def main() -> None:
    src = Image.open(MOCKUP).convert("RGBA")
    assert src.size == (1024, 576), src.size

    # Full mockup reference (Browser stage backdrop option)
    src.convert("RGB").save(OUT / "mockup_full.jpg", quality=95)

    # Logo — tight crop, keep TM, drop streetlamp by taking left-center of logo band
    logo = src.crop((300, 12, 730, 100)).convert("RGBA")
    # Punch pure black
    px = logo.load()
    for y in range(logo.height):
        for x in range(logo.width):
            r, g, b, a = px[x, y]
            if r < 20 and g < 20 and b < 20:
                px[x, y] = (0, 0, 0, 0)
    logo.save(OUT / "logo.png")

    # Blank plates (text scrubbed)
    blank_plate(src, (48, 138, 268, 188), OUT / "btn_idle.png")
    blank_plate(src, (42, 252, 274, 305), OUT / "btn_selected.png")

    # JOIN — scrub "JOIN" letters, keep red plate
    join_box = (760, 470, 1005, 555)
    blank_plate(src, join_box, OUT / "join.png")
    # Also keep labeled join for reference
    src.crop(join_box).save(OUT / "join_labeled.png")

    # Clean alley: heavy blur of mockup + darken, then keep lower cobbles sharp
    bg = src.copy()
    soft = src.filter(ImageFilter.GaussianBlur(18))
    dark = Image.new("RGBA", src.size, (0, 0, 0, 180))
    soft = Image.alpha_composite(soft, dark)
    # Keep bottom cobbles + right lamp area more intact
    cobble = src.crop((0, 430, 1024, 576))
    bg.paste(soft)
    bg.paste(cobble, (0, 430))
    # Right lamp strip
    lamp = src.crop((880, 0, 1024, 430))
    bg.paste(lamp, (880, 0))
    # Left dark wall without buttons — blur + darken
    left = soft.crop((0, 100, 280, 470))
    bg.paste(left, (0, 100))
    bg.convert("RGB").save(OUT / "alley_clean.jpg", quality=93)
    bg.resize((1440, 810), Image.Resampling.LANCZOS).convert("RGB").save(
        OUT / "alley_clean_1440.jpg", quality=93
    )

    print("ok", sorted(p.name for p in OUT.iterdir()))


if __name__ == "__main__":
    main()
