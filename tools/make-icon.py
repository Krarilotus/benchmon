"""Regenerate the checked-in app icons (development only; requires Pillow).

Drawn from simple shapes, without any screenshots or machine-specific information.
"""
from pathlib import Path
from PIL import Image, ImageDraw

root = Path(__file__).resolve().parent.parent
assets = root / 'assets'
assets.mkdir(exist_ok=True)
image = Image.new('RGBA', (256, 256), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)
draw.rounded_rectangle((10, 10, 246, 246), radius=34, fill=(4, 9, 6, 255),
                       outline=(57, 255, 136, 255), width=12)
draw.line([(40, 190), (216, 190)], fill=(20, 70, 40, 255), width=8)
for left, top, color in [(48, 118, (57, 255, 136)), (91, 63, (180, 48, 62)),
                         (134, 96, (255, 164, 58)), (177, 44, (117, 209, 255))]:
    draw.rounded_rectangle((left, top, left + 30, 176), radius=5, fill=(*color, 255))
image.save(assets / 'benchmon.ico', sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
image.resize((64, 64), Image.Resampling.LANCZOS).save(assets / 'benchmon.png')
