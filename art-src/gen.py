"""Alive Kun art generator: MiSTer Kun as the monster on the slab, jolted back to life by a board
everyone had given up for dead.

Same pipeline as the Tasty and Seedy art: MiSTer Kun's paths from the upstream remaster, hand-written
SVG props on top, inkscape for PNGs, ImageMagick for the raw splash.
Run: python3 art-src/gen.py  (writes ../art and ../art/icons)
Needs mister_kun_fullcolor.svg from https://github.com/baxysquare/mister_kun (set KUN_SVG to its path).
"""
import gzip, re, subprocess, os

HERE = os.path.dirname(os.path.abspath(__file__))
ART = os.path.join(HERE, "..", "art")
ICONS = os.path.join(ART, "icons")
KUN_SRC = os.environ.get("KUN_SVG", "/mnt/source/mister-kun-splash/upstream/mister_kun_fullcolor.svg")

src = open(KUN_SRC).read()
PATHS = [p for p in re.findall(r'<path [^>]*/>', src) if 'm-81.806' not in p]
OUTLINE, FILL, FACE = PATHS[0], PATHS[1], PATHS[2:]
FILL_D = re.search(r'd="([^"]*)"', FILL).group(1)
EYE_D = [re.search(r'd="([^"]*)"', p).group(1) for p in FACE if 'fill="#fff"' in p]

PINK = "#e88cb8"; BLACK = "#000"; WHITE = "#efefef"; GREY = "#b3b3b3"
SKIN = "#cfe6bf"     # a freshly revived, faintly green Kun
SKIN_D = "#a9cc95"
ACCENT = "#8fe04a"   # lab-monitor green: the ItsAlive accent
SPARK = "#ffd23f"; SPARK_L = "#fff3b0"
STEEL = "#9aa3ad"; STEEL_D = "#646d78"; STEEL_L = "#d4dae0"
COPPER = "#d9844a"; COPPER_D = "#a35a2a"; WOOD = "#8a5a36"; WOOD_D = "#5e3b22"
RED = "#d9463b"; PAPER = "#fbfbf6"; SCREEN = "#10261a"; SD = "#2f6fd6"
S = 'stroke="#000" stroke-width="16" stroke-linejoin="round" stroke-linecap="round"'
S10 = 'stroke="#000" stroke-width="10" stroke-linejoin="round" stroke-linecap="round"'
FONT = 'font-family="Open Sans" font-weight="800" font-style="italic"'
MONO = 'font-family="DejaVu Sans Mono" font-weight="bold"'
JP = 'font-family="Noto Sans JP Thin" font-weight="800"'

DEFS = f'<defs><clipPath id="body"><path d="{FILL_D}"/></clipPath></defs>'


def bolt(x, y, s=1.0, rot=0, fill=SPARK):
    """A lightning bolt, centred on its middle."""
    return (f'<g transform="translate({x} {y}) rotate({rot}) scale({s})">'
            f'<path d="M24 -100 L-46 10 L-4 10 L-30 100 L52 -16 L8 -16 L36 -100 Z" fill="{fill}" {S10}/></g>')


def sparks(x, y, s=1.0, flip=False):
    """Three little zig-zag sparks fanning out from (x, y)."""
    k = -1 if flip else 1
    out = f'<g transform="translate({x} {y}) scale({k * s} {s})">'
    for d in ("M10 -20 L40 -50 L34 -32 L66 -62", "M18 0 L54 -6 L44 8 L86 2", "M10 20 L40 44 L30 48 L60 74"):
        out += f'<path d="{d}" fill="none" stroke="#000" stroke-width="20" stroke-linecap="round" stroke-linejoin="round"/>'
        out += f'<path d="{d}" fill="none" stroke="{SPARK}" stroke-width="9" stroke-linecap="round" stroke-linejoin="round"/>'
    return out + '</g>'


def stitches(x1, y1, x2, y2, n, w=36):
    """A scar from (x1,y1) to (x2,y2) with n cross stitches."""
    out = f'<path d="M{x1} {y1} L{x2} {y2}" stroke="#000" stroke-width="10" stroke-linecap="round"/>'
    dx, dy = x2 - x1, y2 - y1
    ln = (dx * dx + dy * dy) ** 0.5
    nx, ny = -dy / ln * w / 2, dx / ln * w / 2
    for i in range(n):
        t = (i + 0.5) / n
        cx, cy = x1 + dx * t, y1 + dy * t
        out += f'<path d="M{cx - nx:.1f} {cy - ny:.1f} L{cx + nx:.1f} {cy + ny:.1f}" stroke="#000" stroke-width="9" stroke-linecap="round"/>'
    return out


# --- the monster ----------------------------------------------------------------------------------

def neck_bolt():
    """One bolt, pointing left out of the cheek at (0, 0)."""
    return (f'<rect x="-70" y="-26" width="110" height="52" fill="{STEEL}" {S}/>'
            f'<path d="M-62 -10 L30 -10" stroke="{STEEL_L}" stroke-width="8" stroke-linecap="round"/>'
            f'<path d="M-110 -50 L-70 -50 L-70 50 L-110 50 Z" fill="{STEEL_D}" {S}/>'
            f'<path d="M-104 -24 L-76 -24" stroke="{STEEL}" stroke-width="8" stroke-linecap="round"/>')


def neck_bolts():
    """Bolts through the cheeks, drawn behind the body so they come out of its sides."""
    return (f'<g transform="translate(0 618)">{neck_bolt()}</g>'
            f'<g transform="translate(1000 618) scale(-1 1)">{neck_bolt()}</g>')


def flattop():
    """A flat-top with a jagged fringe, inside the head and between the ears."""
    fringe = "M40 150 L960 150 L960 248 "
    xs = list(range(960, 39, -46))
    for i, x in enumerate(xs):
        fringe += f"L{x} {248 if i % 2 == 0 else 292} "
    fringe += "Z"
    hair = f'<path d="{fringe}" fill="#1b1b1b" stroke="#000" stroke-width="8" stroke-linejoin="round"/>'
    # a little shine on top
    hair += f'<path d="M400 222 L600 222" stroke="#4a4a4a" stroke-width="12" stroke-linecap="round"/>'
    # keep the ears clear of the hair: they stand above y=191
    ears = '<path d="M0 0 L1000 0 L1000 191 L0 191 Z"/>'
    return (f'<defs><clipPath id="hair"><path d="M0 191 L336 191 L336 120 L664 120 L664 191 L1000 191 L1000 400 L0 400 Z"/></clipPath></defs>'
            f'<g clip-path="url(#body)"><g clip-path="url(#hair)">{hair}</g></g>')


def scars():
    out = stitches(540, 330, 770, 346, 6)               # across the forehead, under the fringe
    out += stitches(150, 700, 250, 790, 3, 30)           # on the left cheek
    return out


def kun(eyes="sleepy", mouth=True):
    face = "".join(FACE)
    # recolour Kun's white to a faint green complexion; the eye whites stay white
    fill = FILL.replace('fill="#efefef"', f'fill="{SKIN}"')
    out = f'<g id="kun">{neck_bolts()}{OUTLINE}{fill}{flattop()}{face}{scars()}'
    if eyes == "wide":
        # shocked awake: the lids fly open and the pupils shrink to pinpoints
        for cx in (322, 678):
            out += f'<circle cx="{cx}" cy="430" r="84" fill="#fff" stroke="#000" stroke-width="18"/>'
            out += f'<circle cx="{cx + (12 if cx < 500 else -12)}" cy="436" r="17" fill="#000"/>'
    return out + '</g>'


# --- props (centred on 0,0, about +-150) ------------------------------------------------------------

PROP = {}

PROP["lever"] = f'''<g>
   <rect x="-130" y="40" width="260" height="110" rx="10" fill="{WOOD}" {S}/>
   <path d="M-110 70 L110 70 M-110 100 L80 100" stroke="{WOOD_D}" stroke-width="8" stroke-linecap="round"/>
   <rect x="-96" y="10" width="44" height="60" rx="6" fill="{COPPER}" {S10}/>
   <rect x="52" y="10" width="44" height="60" rx="6" fill="{COPPER}" {S10}/>
   <path d="M-74 30 L60 -130" stroke="#000" stroke-width="40" stroke-linecap="round"/>
   <path d="M-74 30 L60 -130" stroke="{COPPER}" stroke-width="22" stroke-linecap="round"/>
   <path d="M44 -112 L106 -164" stroke="#000" stroke-width="56" stroke-linecap="round"/>
   <path d="M44 -112 L106 -164" stroke="{RED}" stroke-width="38" stroke-linecap="round"/>
   <circle cx="-74" cy="30" r="20" fill="{STEEL}" {S10}/>
   {sparks(96, 20, 0.8)}
 </g>'''

PROP["monitor"] = f'''<g transform="rotate(-6)">
   <path d="M-40 120 L40 120 L60 160 L-60 160 Z" fill="{STEEL_D}" {S}/>
   <rect x="-170" y="-140" width="340" height="270" rx="26" fill="{STEEL}" {S}/>
   <rect x="-140" y="-112" width="280" height="200" rx="14" fill="{SCREEN}" {S10}/>
   <text x="-118" y="-62" {MONO} font-size="30" fill="{ACCENT}">INSTALLING</text>
   <text x="-118" y="-22" {MONO} font-size="30" fill="{ACCENT}">DO NOT</text>
   <text x="-118" y="18" {MONO} font-size="30" fill="{ACCENT}">POWER OFF</text>
   <rect x="-118" y="40" width="236" height="26" fill="none" stroke="{ACCENT}" stroke-width="5"/>
   <rect x="-112" y="46" width="150" height="14" fill="{ACCENT}"/>
   <circle cx="138" cy="110" r="8" fill="{ACCENT}" stroke="#000" stroke-width="5"/>
 </g>'''

PROP["hdmi"] = f'''<g transform="rotate(-24)">
   <path d="M0 110 C0 170 -60 180 -120 170" fill="none" stroke="#000" stroke-width="54" stroke-linecap="round"/>
   <path d="M0 110 C0 170 -60 180 -120 170" fill="none" stroke="#2b2b2b" stroke-width="34" stroke-linecap="round"/>
   <rect x="-70" y="-30" width="140" height="150" rx="20" fill="#2b2b2b" {S}/>
   <path d="M-50 10 L50 10 M-50 50 L50 50 M-50 90 L50 90" stroke="#3d3d3d" stroke-width="10" stroke-linecap="round"/>
   <path d="M-96 -150 L96 -150 L96 -96 L70 -30 L-70 -30 L-96 -96 Z" fill="{STEEL_L}" {S}/>
   <path d="M-70 -120 L70 -120 L70 -98 L54 -62 L-54 -62 L-70 -98 Z" fill="#000"/>
   <path d="M-52 -106 L52 -106" stroke="{SPARK}" stroke-width="10" stroke-dasharray="8 8"/>
   <text x="0" y="78" {FONT} font-size="30" text-anchor="middle" fill="{ACCENT}">HDMI</text>
   {sparks(70, -150, 0.9)}{sparks(-70, -150, 0.9, flip=True)}
 </g>'''

PROP["sdcard"] = f'''<g transform="rotate(12)">
   <path d="M-110 -150 L60 -150 L110 -100 L110 150 L-110 150 Z" fill="{SD}" {S}/>
   {"".join(f'<rect x="{x}" y="-136" width="20" height="54" rx="4" fill="{SPARK}" stroke="#000" stroke-width="6"/>' for x in (-90, -60, -30, 0, 30))}
   <rect x="-86" y="-50" width="172" height="150" rx="8" fill="{PAPER}" {S10}/>
   <text x="0" y="0" {FONT} font-size="40" text-anchor="middle" fill="#000">MiSTer</text>
   <text x="0" y="66" {FONT} font-size="54" text-anchor="middle" fill="{SD}">SD</text>
   <g transform="rotate(-34 0 30)">
     <rect x="-150" y="6" width="300" height="56" rx="14" fill="#f0d2a8" {S10}/>
     <rect x="-36" y="6" width="72" height="56" fill="#e2bd8a" stroke="#000" stroke-width="8"/>
     {"".join(f'<circle cx="{x}" cy="{y}" r="5" fill="#b78c56"/>' for x in (-110, -80, 80, 110) for y in (24, 44))}
   </g>
 </g>'''

PROP["power"] = f'''<g transform="rotate(-8)">
   <rect x="-130" y="-130" width="260" height="260" rx="24" fill="#2b2b3a" {S}/>
   <circle r="78" fill="{RED}" {S}/>
   <path d="M-32 -38 A50 50 0 1 0 32 -38" fill="none" stroke="#fff" stroke-width="16" stroke-linecap="round"/>
   <path d="M0 -60 L0 -6" stroke="#fff" stroke-width="16" stroke-linecap="round"/>
   <g transform="rotate(34)">
     <rect x="-170" y="-26" width="340" height="52" fill="{SPARK}" {S10}/>
     <path d="M-130 -26 L-100 26 M-70 -26 L-40 26 M40 -26 L70 26 M100 -26 L130 26" stroke="#000" stroke-width="14"/>
   </g>
   <g transform="rotate(-24)">
     <rect x="-170" y="-38" width="340" height="76" fill="{SPARK}" {S10}/>
     <text x="0" y="18" {FONT} font-size="50" text-anchor="middle" fill="#000">DON'T!</text>
   </g>
 </g>'''

PROP["chip"] = f'''<g transform="rotate(-10)">
   {"".join(f'<path d="M{x} -150 L{x} -110 M{x} 150 L{x} 110 M-150 {x} L-110 {x} M150 {x} L110 {x}" stroke="#000" stroke-width="26" stroke-linecap="round"/><path d="M{x} -148 L{x} -110 M{x} 148 L{x} 110 M-148 {x} L-110 {x} M148 {x} L110 {x}" stroke="{GREY}" stroke-width="12" stroke-linecap="round"/>' for x in (-70, -24, 24, 70))}
   <rect x="-118" y="-118" width="236" height="236" rx="18" fill="#2b2b3a" {S}/>
   <circle cx="-80" cy="-80" r="10" fill="{GREY}"/>
   <text x="0" y="-6" {FONT} font-size="46" text-anchor="middle" fill="#fff">ADV</text>
   <text x="0" y="52" {FONT} font-size="46" text-anchor="middle" fill="{ACCENT}">7513</text>
 </g>'''

PROP["heartbeat"] = f'''<g transform="rotate(6)">
   <rect x="-170" y="-130" width="340" height="250" rx="22" fill="{STEEL}" {S}/>
   <rect x="-144" y="-104" width="288" height="168" rx="10" fill="{SCREEN}" {S10}/>
   <path d="M-144 -20 L144 -20 M-144 30 L144 30 M-80 -104 L-80 64 M0 -104 L0 64 M80 -104 L80 64" stroke="#1f4a30" stroke-width="4"/>
   <path d="M-136 0 L-70 0 L-50 -20 L-30 0 L-10 0 L10 -84 L34 52 L54 0 L136 0" fill="none" stroke="{ACCENT}" stroke-width="11" stroke-linejoin="round" stroke-linecap="round"/>
   <circle cx="136" cy="0" r="10" fill="#fff"/>
   <circle cx="-120" cy="92" r="12" fill="{RED}" stroke="#000" stroke-width="6"/>
   <text x="40" y="104" {MONO} font-size="28" text-anchor="middle" fill="#000">ALIVE</text>
 </g>'''

PROP["tesla"] = f'''<g>
   <rect x="-130" y="110" width="260" height="44" rx="10" fill="{WOOD}" {S}/>
   <path d="M-70 120 C-70 20 -40 -60 -30 -150" fill="none" stroke="#000" stroke-width="30" stroke-linecap="round"/>
   <path d="M70 120 C70 20 40 -60 30 -150" fill="none" stroke="#000" stroke-width="30" stroke-linecap="round"/>
   <path d="M-70 120 C-70 20 -40 -60 -30 -150" fill="none" stroke="{COPPER}" stroke-width="14" stroke-linecap="round"/>
   <path d="M70 120 C70 20 40 -60 30 -150" fill="none" stroke="{COPPER}" stroke-width="14" stroke-linecap="round"/>
   <path d="M-40 -90 L-14 -110 L-4 -84 L14 -104 L40 -90" fill="none" stroke="#000" stroke-width="22" stroke-linejoin="round" stroke-linecap="round"/>
   <path d="M-40 -90 L-14 -110 L-4 -84 L14 -104 L40 -90" fill="none" stroke="#b9e9ff" stroke-width="10" stroke-linejoin="round" stroke-linecap="round"/>
   <path d="M-60 0 L-30 -24 L-12 6 L14 -22 L30 6 L60 0" fill="none" stroke="#000" stroke-width="22" stroke-linejoin="round" stroke-linecap="round"/>
   <path d="M-60 0 L-30 -24 L-12 6 L14 -22 L30 6 L60 0" fill="none" stroke="#b9e9ff" stroke-width="10" stroke-linejoin="round" stroke-linecap="round"/>
 </g>'''

PROP["zap"] = f'<g>{bolt(0, 0, 1.5, 12)}</g>'

ICON_ONLY = {
    "stitches": f'<g transform="rotate(-20)"><rect x="-160" y="-60" width="320" height="120" rx="30" fill="{SKIN}" {S}/>{stitches(-120, 0, 120, 0, 5, 70)}</g>',
    "neckbolt": f'<g transform="translate(70 0) scale(2)">{neck_bolt()}</g>',
}


def with_prop(name, x=880, y=830, s=1.25, **kw):
    return DEFS + kun(**kw) + f'<g transform="translate({x} {y}) scale({s})">{PROP[name]}</g>'


def zapped():
    """Fresh off the slab: wide eyes, sparks off both bolts, lightning overhead."""
    return (DEFS + kun(eyes="wide") + sparks(-110, 618, 1.0, flip=True) + sparks(1110, 618, 1.0)
            + bolt(70, 80, 1.1, -14) + bolt(940, 60, 0.9, 16))


# --- output helpers (same as the Tasty and Seedy pipeline) ----------------------------------------------

def svg(w, h, body, vb=None, bg=None):
    vb = vb or f"0 0 {w} {h}"
    b = f'<rect x="-10000" y="-10000" width="20000" height="20000" fill="{bg}"/>' if bg else ""
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{vb}">{b}{body}</svg>'


def write(name, text, d=ART):
    p = os.path.join(d, name + ".svg")
    open(p, "w").write(text)
    return p


def png(svgp, w, pngname=None, d=ART):
    pn = os.path.join(d, (pngname or os.path.basename(svgp)[:-4]) + ".png")
    subprocess.run(["inkscape", svgp, "--export-type=png", f"--export-filename={pn}", f"--export-width={w}"],
                   check=True, capture_output=True)
    return pn


def nested(inner, x, y, size, vb="0 0 1000 1000"):
    return f'<svg x="{x}" y="{y}" width="{size}" height="{size}" viewBox="{vb}">{inner}</svg>'


def wordmark(x, y, scale=1.0, jp=True, dark=False):
    ink = "#f0efef" if dark else "#000"
    t = f'<g transform="translate({x} {y}) scale({scale})">'
    t += f'<text x="0" y="0" {FONT} font-size="120" fill="{ink}" letter-spacing="-2">MiSTer</text>'
    t += (f'<text x="0" y="150" {FONT} font-size="150" fill="{ACCENT}" stroke="{ink}" stroke-width="10" '
          f'paint-order="stroke" letter-spacing="2">IT\'S ALIVE</text>')
    if jp:
        t += f'<text x="8" y="235" {JP} font-size="62" fill="{ink}">ミスター・イッツアライブ</text>'
    return t + '</g>'


def props_row(x, y, size, gap, names):
    out = ""
    for i, n in enumerate(names):
        out += f'<g transform="translate({x + i * (size + gap) + size / 2} {y + size / 2}) scale({size / 340})">{PROP[n]}</g>'
    return out


TAGLINE = "A picture, with no Main_MiSTer."
VARIANTS = ["lever", "monitor", "hdmi", "sdcard", "power", "chip", "heartbeat", "tesla"]
VB = "-130 -20 1260 1120"   # room for the neck bolts and the prop
LAB = "#121a24"


def main():
    os.makedirs(ICONS, exist_ok=True)
    made = []
    p = write("alive-kun", svg(1000, 1000, DEFS + kun(), vb="-130 -70 1260 1260")); made.append(png(p, 512))
    p = write("alive-kun-zapped", svg(1080, 1080, zapped(), vb="-210 -60 1420 1420")); made.append(png(p, 512))
    for v in VARIANTS:
        p = write(f"alive-kun-{v}", svg(1080, 1080, with_prop(v), vb=VB)); made.append(png(p, 512))

    # social preview 1280x640
    body = nested(zapped(), 30, 50, 540, vb="-210 -60 1420 1420")
    body += wordmark(610, 210, 0.86)
    body += f'<text x="616" y="500" {FONT} font-size="40" fill="#000">{TAGLINE}</text>'
    body += "".join(f'<rect x="{616 + i * 150}" y="540" width="150" height="28" fill="{c}"/>'
                    for i, c in enumerate([ACCENT, SKIN, "#000", SPARK]))
    p = write("social-preview", svg(1280, 640, body, bg="#ffffff")); made.append(png(p, 1280))

    # README banner 1280x420, dark
    body = nested(DEFS + kun(), 30, 20, 380, vb="-130 -70 1260 1260")
    body += wordmark(430, 150, 0.62, jp=False, dark=True)
    body += f'<text x="436" y="316" {JP} font-size="36" fill="#f0efef">ミスター・イッツアライブ</text>'
    body += props_row(900, 96, 56, 12, ["lever", "hdmi", "monitor", "heartbeat", "zap"])
    body += f'<text x="902" y="262" {FONT} font-size="30" fill="#f0efef">HDMI and a picture</text>'
    body += f'<text x="902" y="304" {FONT} font-size="30" fill="#f0efef">from a board left</text>'
    body += f'<text x="902" y="346" {FONT} font-size="30" fill="{ACCENT}">for dead.</text>'
    p = write("banner", svg(1280, 420, body, bg=LAB)); made.append(png(p, 1280))

    # installer splash 1280x720: the picture itsalive itself puts on the screen
    body = nested(DEFS + kun(eyes="wide") + sparks(-110, 618, 1.0, flip=True) + sparks(1110, 618, 1.0),
                  30, 80, 560, vb="-210 -60 1420 1420")
    body += wordmark(600, 250, 0.7, jp=False, dark=True)
    body += f'<text x="604" y="430" {FONT} font-size="52" fill="#f0efef">Installing MiSTer...</text>'
    body += f'<text x="604" y="500" {FONT} font-size="52" fill="{SPARK}">Do not power off.</text>'
    p = write("splash", svg(1280, 720, body, bg=LAB)); pn = png(p, 1280); made.append(pn)
    raw = subprocess.run(["magick", pn, "-background", LAB, "-alpha", "remove", "-depth", "8", "BGRA:-"],
                         check=True, capture_output=True).stdout
    assert len(raw) == 1280 * 720 * 4
    with gzip.GzipFile(os.path.join(ART, "splash.raw.gz"), "wb", 9, mtime=0) as f:
        f.write(raw)
    made.append(os.path.join(ART, "splash.raw.gz"))

    # heading icons
    tmp = os.path.join(HERE, "out-scratch")
    os.makedirs(tmp, exist_ok=True)
    icons = dict(PROP)
    icons.update(ICON_ONLY)
    for name, g in icons.items():
        sc = {"stitches": 0.9, "neckbolt": 1.3, "zap": 1.1}.get(name, 1.3)
        body = DEFS + f'<g transform="translate(250 270) scale({sc})">{g}</g>'
        p = write(f"icon-{name}", svg(500, 540, body), d=tmp)
        made.append(png(p, 48, f"{name}-48", d=ICONS))
    print("\n".join(made))


# --- 8-bit Alive Kun: the upstream 32x32 pixel Kun, repainted by hand ------------------------------

KUN8 = """\
.........K............K.........
........KWK..........KWK........
.......KWKWK........KWKWK.......
......KWKGKWK......KWKGKWK......
.....KWKKKKKWKKKKKKWKKKKKWK.....
....KWWWWWWWWWWWWWWWWWWWWWWK....
....KWWWWWWWWWWWWWWWWWWWWWWK....
...KWWWWWWWWWWWWWWWWWWWWWWWWK...
...KWWWWWWWWWWWWWWWWWWWWWWWWK...
...KWWWWWWWWWWWWWWWWWWWWWWWWK...
...KWWWWWWWWWWWWWWWWWWWWWWWWK...
...KWWWWWWWWWKWWWWKWWWWWWWWWK...
...KWKKKKKKKKKKKKKKKKKKKKKKWK...
...KWKWWWKKKKWKPPKWKKKKWWWKWK...
..KKWKWWWWKKWWKPPKWWKKWWWWKWKK..
.KWWWKWWWWWWWWKKKKWWWWWWWWKWWWK.
.KWWWWKWWWWWWKKRRKKWWWWWWKWWWWK.
KWWWWWWKKKKKKPPKKPPKKKKKKWWWWWWK
KWWWWWWWWWWKPPPPPPPPKWWWWWWWWWWK
KWWWWWWWWWWWKPPKKPPKWWWWWWWWWWWK
.KWWWWWWWWWKWKKPPKKWKWWWWWWWWWK.
.KWWWWWWWWWKWWWKKWWWKWWWWWWWWWK.
..KKWWWWWWWWWWWWWWWWWWWWWWWWKK..
...KWWWWWWWWWWWWWWWWWWWWWWWWK...
...KWWWWWWWWWWWWWWWWWWWWWWWWK...
...KWWWWWWWWWWWWWWWWWWWWWWWWK...
..KWWWWWWWWWWWKKKKWWWWWWWWWWWK..
.KWWWWWWWWWWWK....KWWWWWWWWWWWK.
.KWWWWWWWWWWK......KWWWWWWWWWWK.
..KWWWWWWWWWK......KWWWWWWWWWK..
...KKKKKKKKK........KKKKKKKKK...
"""
PAL8 = {"K": "#010101", "W": SKIN, "w": "#ffffff", "G": "#e98db8", "P": "#b4b4b4", "R": "#e177af",
        "H": "#1b1b1b", "B": STEEL, "b": STEEL_D, "Y": SPARK}


def kun8():
    g = [list(r) for r in KUN8.splitlines()]
    g.insert(0, list("." * 32))   # the file starts one row down, like the upstream grid
    def put(x, y, c):
        g[y][x] = c
    # eye whites stay white
    for y in range(14, 18):
        for x in range(32):
            if g[y][x] == "W" and (5 <= x <= 13 or 18 <= x <= 26):
                put(x, y, "w")
    # flat-top: rows 6..8 across the head, a ragged fringe on row 9
    for y in (6, 7, 8):
        for x in range(32):
            if g[y][x] == "W":
                put(x, y, "H")
    for x in range(5, 27, 2):
        if g[9][x] == "W":
            put(x, 9, "H")
    # forehead scar with stitches
    for x in range(18, 25):
        put(x, 10, "K")
    for x in (20, 23):
        put(x, 9, "K"); put(x, 11, "K")
    # neck bolts out of both cheeks
    for (x, c) in ((0, "b"), (1, "B"), (31, "b"), (30, "B")):
        put(x, 21, c); put(x, 22, c)
    put(1, 21, "B"); put(30, 21, "B")
    rects = "".join(f'<rect x="{x}" y="{y}" width="1" height="1" fill="{PAL8[c]}"/>'
                    for y, row in enumerate(g) for x, c in enumerate(row) if c != ".")
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32" shape-rendering="crispEdges">{rects}</svg>'


def main8():
    p = write("alive-kun-8bit-32x32", kun8())
    png(p, 32)
    print(png(p, 512, "alive-kun-8bit"))


if __name__ == "__main__":
    main()
    main8()
