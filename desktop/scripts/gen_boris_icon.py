"""Generate Boris app icon PNG from the classic waveform mark (source for `tauri icon`)."""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "app-icon.png"
PREVIEW = ROOT / "public" / "icons" / "boris-icon-256.png"
SIZE = 1024

# Classic boris-mark bar geometry in a 24×24 viewBox (stroke bars).
# Each entry is (x_center, y_top, y_bottom) in 24-space.
BARS = [
    (2, 10, 13),
    (6, 6, 17),
    (10, 3, 21),
    (14, 8, 15),
    (18, 5, 18),
    (22, 10, 13),
]


def main() -> None:
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))

    # Rounded dark plate — matches app chrome
    bg = (12, 13, 16, 255)
    radius = int(SIZE * 0.22)
    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, SIZE - 1, SIZE - 1], radius=radius, fill=255
    )
    plate = Image.new("RGBA", (SIZE, SIZE), bg)
    img = Image.composite(plate, img, mask)
    d = ImageDraw.Draw(img)

    # Map 24×24 mark into a padded square inside the plate
    pad = SIZE * 0.22
    scale = (SIZE - 2 * pad) / 24.0
    stroke = max(8, int(SIZE * 0.055))
    color = (250, 250, 250, 255)  # #fafafa — same as the SVG mark

    for x, y0, y1 in BARS:
        cx = pad + x * scale
        top = pad + y0 * scale
        bot = pad + y1 * scale
        # Capsule bar (round line ends)
        d.rounded_rectangle(
            [cx - stroke / 2, top, cx + stroke / 2, bot],
            radius=stroke / 2,
            fill=color,
        )

    OUT.parent.mkdir(parents=True, exist_ok=True)
    img.save(OUT, "PNG")
    print("wrote", OUT, img.size)

    PREVIEW.parent.mkdir(parents=True, exist_ok=True)
    img.resize((256, 256), Image.Resampling.LANCZOS).save(PREVIEW)
    print("wrote", PREVIEW)


if __name__ == "__main__":
    main()
