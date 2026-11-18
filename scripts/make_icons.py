import os, sys
from PIL import Image, ImageDraw

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = Image.open(sys.argv[1]).convert("RGB")
box = src.convert("L").point(lambda v: 255 if v > 14 else 0).getbbox()
w, h = box[2] - box[0], box[3] - box[1]
k = 4
mask = Image.new("L", (w * k, h * k))
ImageDraw.Draw(mask).rounded_rectangle((0, 0, w * k - 1, h * k - 1), round(min(w, h) * k * 0.24), fill=255)
side = max(w, h)
img = Image.new("RGBA", (side, side))
img.paste(src.crop(box).convert("RGBA"), ((side - w) // 2, (side - h) // 2), mask.resize((w, h), Image.LANCZOS))
img = img.resize((256, 256), Image.LANCZOS)
os.makedirs(os.path.join(root, "assets"), exist_ok=True)
img.save(os.path.join(root, "ui", "icons", "app.png"), optimize=True)
img.save(os.path.join(root, "assets", "app.ico"), sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
img.resize((32, 32), Image.LANCZOS).save(os.path.join(root, "ui", "icons", "fav.png"), optimize=True)
