#!/usr/bin/env python3
"""Generate a 16x16 CJK font bitmap for common Chinese characters (U+4E00 - U+9FFF subset)."""
import struct
from PIL import Image, ImageFont, ImageDraw

# Try to find a CJK system font
import subprocess
result = subprocess.run(['fc-list', ':lang=zh', 'file'], capture_output=True, text=True)
font_paths = [l.split(':')[0].strip() for l in result.stdout.strip().split('\n') if l.strip()]
# Fallback fonts
for fp in font_paths + [
    '/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc',
    '/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf',
    '/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc',
    '/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc',
    '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf',
]:
    try:
        font = ImageFont.truetype(fp, 16)
        print(f"Using font: {fp}")
        break
    except Exception:
        continue
else:
    print("No CJK font found, using default")
    font = ImageFont.load_default()

# Generate glyphs for CJK Unified Ideographs (U+4E00 - U+6FFF: first 8192 chars)
START = 0x4E00
COUNT = 8192
output = bytearray()

for i in range(COUNT):
    codepoint = START + i
    char = chr(codepoint)
    
    # Render to 16x16 bitmap
    img = Image.new('L', (16, 16), 0)
    draw = ImageDraw.Draw(img)
    try:
        # Center the character in 16x16 box
        bbox = draw.textbbox((0, 0), char, font=font)
        tw = bbox[2] - bbox[0]
        th = bbox[3] - bbox[1]
        x = (16 - tw) // 2 - bbox[0]
        y = (16 - th) // 2 - bbox[1]
        draw.text((x, y), char, font=font, fill=255)
    except Exception:
        pass  # Leave as empty glyph
    
    # Convert to 16x16=256 bits = 32 bytes (row-major, MSB first per row)
    glyph = bytearray(32)
    for row in range(16):
        val = 0
        for col in range(16):
            if img.getpixel((col, row)) > 128:
                val |= 1 << (7 - (col % 8 if col < 8 else col - 8))
            if col == 7:
                glyph[row * 2] = val
                val = 0
            elif col == 15:
                glyph[row * 2 + 1] = val
    output.extend(glyph)
    
    if i % 2000 == 0:
        print(f"  Generated {i}/{COUNT} glyphs...")

# Write font file
out_path = 'kernel/src/arch/x86_64/font16x16_cjk.bin'
with open(out_path, 'wb') as f:
    f.write(output)

print(f"CJK font written: {out_path} ({len(output)} bytes, {COUNT} glyphs)")
