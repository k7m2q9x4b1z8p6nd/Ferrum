import os, random, sys
from PIL import Image, ImageDraw, ImageFilter

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
rnd = random.Random(7)


def scene(w, h, seed):
    r = random.Random(seed)
    img = Image.effect_noise((w // 8, h // 8), 90).convert("RGB").resize((w, h), Image.BICUBIC)
    d = ImageDraw.Draw(img)
    for _ in range(60):
        x, y = r.randrange(w), r.randrange(h)
        s = r.randrange(40, 400)
        d.ellipse((x, y, x + s, y + s), fill=tuple(r.randrange(256) for _ in range(3)))
    return img.filter(ImageFilter.GaussianBlur(1.2))


for i in range(24):
    scene(2400, 1600, i).save(os.path.join(out, f"photo{i}.jpg"), quality=85)
    scene(1600, 1000, 100 + i).save(os.path.join(out, f"photo{i}.png"))

for g in range(4):
    frames = []
    for f in range(40):
        im = Image.effect_noise((100, 75), 60).convert("RGB").resize((500, 375), Image.BICUBIC)
        d = ImageDraw.Draw(im)
        for k in range(8):
            x = (f * 12 + k * 60) % 500
            d.ellipse((x, 40 + k * 35, x + 60, 100 + k * 35), fill=(rnd.randrange(256), rnd.randrange(256), rnd.randrange(256)))
        frames.append(im.quantize(256))
    frames[0].save(os.path.join(out, f"anim{g}.gif"), save_all=True, append_images=frames[1:], duration=40, loop=0)

names = [f"photo{i}.jpg" for i in range(24)] + [f"photo{i}.png" for i in range(24)] + [f"anim{g}.gif" for g in range(4)]
print(sum(os.path.getsize(os.path.join(out, n)) for n in names) // 1024, "KiB")
