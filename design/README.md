# Sigiro Design Assets

## Directory structure

```
design/
  logo/
    mark.svg          — signal spike mark, terra cotta on transparent
    mark-white.svg    — white version for dark backgrounds
  tokens/
    brand.json        — colors, typography, logo path data
  paper/
    README.md         — Paper design file notes
  README.md           — this file
```

## Logo

**Chosen direction: B — Signal Spike**

The mark is an EKG-style signal trace: dip first, then spike, on a centered baseline.
When rotated 90° CCW it reads as an S — built for loading/transition animations.

```
CSS animation: transform: rotate(-90deg)
transform-origin: center
transform-box: fill-box
```

The path `M2 16 L8 16 L12 26 L18 16 L22 6 L28 16 L34 16` uses:
- Baseline at y=16 (exact center of 36×32 viewBox)
- Dip: −10px to y=26
- Spike: +10px to y=6
- Equal arms → balanced S after rotation

The anomaly dot at `(22, 6)` marks the spike apex. After 90° CCW rotation it lands near the S crossing point.

## Colors

| Role         | Hex       |
|--------------|-----------|
| Accent       | `#C4501A` |
| Near-black   | `#1C1916` |
| Off-white    | `#F7F4EF` |
| Warm mid     | `#9A9188` |
| Warm light   | `#EDE9E2` |
| Warm border  | `#D5D0C9` |

## Typography

- **Display**: Playfair Display 700, tracking −0.03em
- **UI / body**: Inter, tracking −0.02em
- **Labels / caps**: Inter 600, tracking +0.08em, uppercase

## Paper design file

See `design/paper/README.md` for the design file snapshot and artboard inventory.
