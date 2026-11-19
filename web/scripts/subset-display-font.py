"""Subset the display face to the characters used in web/src.

Run: uv run --with fonttools --with brotli python scripts/subset-display-font.py
Reads ../assets/fonts/ResourceHanRoundedCN-{700,900}.otf and writes
src/assets/fonts/display-{700,900}.woff2.

shortcut: glyphs cover only characters present in the source tree, rerun after adding new display copy. Switch to unicode-range chunks of GB2312 if user content needs the display face.
"""

from pathlib import Path

from fontTools import subset

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"
FONTS = ROOT.parent / "assets" / "fonts"
OUT = SRC / "assets" / "fonts"


def collect_chars() -> str:
    chars = set(chr(c) for c in range(0x20, 0x7F))
    chars.update("，。、！？：；“”‘’（）《》·…—～")
    for path in SRC.rglob("*"):
        if path.suffix not in {".ts", ".tsx", ".css", ".html"}:
            continue
        for ch in path.read_text(encoding="utf-8"):
            if ord(ch) > 0x2E7F:
                chars.add(ch)
    return "".join(sorted(chars))


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    text = collect_chars()
    for weight in (700, 900):
        options = subset.Options()
        options.flavor = "woff2"
        options.layout_features = ["*"]
        options.name_IDs = ["*"]
        options.desubroutinize = True
        font = subset.load_font(str(FONTS / f"ResourceHanRoundedCN-{weight}.otf"), options)
        subsetter = subset.Subsetter(options)
        subsetter.populate(text=text)
        subsetter.subset(font)
        target = OUT / f"display-{weight}.woff2"
        subset.save_font(font, str(target), options)
        print(f"{target.relative_to(ROOT)}: {len(text)} chars, {target.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
