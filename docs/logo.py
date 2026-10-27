#!/usr/bin/env python3
"""Generate the uptime-kinomi logo and favicon (standard library only).

A blackberry as a round sticker: half-spheres in shades of purple, placed
at the 32 face centres of a soccer ball (truncated icosahedron) with a
hexagon facing the viewer, a white pulse in front, on a dark grid "screen".

    python3 docs/logo.py      # writes docs/logo.svg and assets/favicon.svg

The favicon drops the shadow and the grid and uses a thicker pulse, so it
stays readable at 16 px.
"""
import math
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

INK = "#1a0826"
SHADES = ["#3b1457", "#5a2183", "#7433a6", "#4a1a6e", "#8a4cc0", "#62268f"]
C = 64                         # centre
# Outer radius of the sticker and width of the white pulse; the white ring
# around the screen is as wide as the pulse.
LOGO = (60, 2.3)
FAVICON = (62, 4.6)
PULSE = "M0,64H36L41,56L46,64H51L58,38L66,90L72,56L77,64H128"
PHI = (1 + 5 ** 0.5) / 2
TITLE = "uptime-kinomi logo: a blackberry with a pulse line"


def norm(v):
    n = math.sqrt(sum(c * c for c in v))
    return tuple(c / n for c in v)


def perms(v):
    x, y, z = v
    return [(x, y, z), (z, x, y), (y, z, x)]


def face_centres():
    """Unit vectors to the 12 pentagons (icosahedron vertices) and the 20
    hexagons (vertices of the dual dodecahedron) of a soccer ball, and the
    hexagon that will face the viewer."""
    pent, hexa = set(), set()
    for s1 in (1, -1):
        for s2 in (1, -1):
            for p in perms((0, s1, s2 * PHI)):
                pent.add(norm(p))
    for s1 in (1, -1):
        for s2 in (1, -1):
            for s3 in (1, -1):
                hexa.add(norm((s1, s2, s3)))
            for p in perms((0, s1 * PHI, s2 / PHI)):
                hexa.add(norm(p))
    hexa = list(hexa)
    return list(pent) + hexa, hexa[0]


def to_front(pts, v):
    """Rotate all points so that v points at the viewer (+z)."""
    ax = (v[1], -v[0], 0.0)    # v x z
    s, c = math.hypot(ax[0], ax[1]), v[2]
    ax = (ax[0] / s, ax[1] / s, 0.0)

    def rot(p):                # Rodrigues' rotation formula
        d = ax[0] * p[0] + ax[1] * p[1]
        cr = (ax[1] * p[2], -ax[0] * p[2], ax[0] * p[1] - ax[1] * p[0])
        return tuple(p[k] * c + cr[k] * s + ax[k] * d * (1 - c) for k in range(3))
    return [rot(p) for p in pts]


def berry(ri):
    pts, front = face_centres()
    pts = to_front(pts, front)
    # all neighbours are equally far apart: bumps just touch
    a = min(math.acos(max(-1, min(1, sum(x * y for x, y in zip(pts[0], q))))) for q in pts[1:]) / 2 * 1.02
    R = 0.9 * ri / (1 + math.sin(a))   # the berry fills 90 % of the screen height
    rb = R * math.sin(a)       # radius of a half-sphere
    g = [f'<circle cx="{C}" cy="{C}" r="{R:.1f}" fill="{INK}"/>']
    for i in sorted(range(len(pts)), key=lambda i: pts[i][2]):   # back to front
        x, y, z = pts[i]
        if z < -math.sin(a):
            continue           # hidden behind the sphere
        bx, by = C + R * math.cos(a) * x, C + R * math.cos(a) * y
        rr = max(rb * abs(z), 0.01)
        # local frame, +x radially outward: the dome's circular outline on
        # the outer side, the foreshortened base ellipse on the inner side
        g.append(f'<path d="M0,{-rb:.2f}A{rb:.2f},{rb:.2f} 0 0 1 0,{rb:.2f}A{rr:.2f},{rb:.2f} 0 0 1 0,{-rb:.2f}Z" '
                 f'transform="translate({bx:.1f} {by:.1f}) rotate({math.degrees(math.atan2(y, x)):.1f})" '
                 f'fill="{SHADES[(i * 5 + 1) % len(SHADES)]}" stroke="{INK}" stroke-width="1.6"/>')
    return "".join(g)


def pulse(width):
    return (f'<path d="{PULSE}" fill="none" stroke="{INK}" stroke-width="{2 * width:.1f}" '
            'stroke-linejoin="round" stroke-linecap="round"/>'
            f'<path d="{PULSE}" fill="none" stroke="#fff" stroke-width="{width:.1f}" '
            'stroke-linejoin="round" stroke-linecap="round"/>')


def svg(favicon):
    outer, width = FAVICON if favicon else LOGO
    ri = outer - width             # radius of the dark screen
    parts = ['<defs>']
    if not favicon:
        parts.append('<filter id="s" x="-20%" y="-20%" width="140%" height="140%">'
                     '<feGaussianBlur stdDeviation="1.8"/></filter>')
    parts.append(f'<clipPath id="c"><circle cx="{C}" cy="{C}" r="{ri:.1f}"/></clipPath></defs>')
    if not favicon:
        parts.append(f'<circle cx="{C + 1.5}" cy="{C + 3}" r="{outer}" fill="#000" opacity=".3" filter="url(#s)"/>')
    parts.append(f'<circle cx="{C}" cy="{C}" r="{outer}" fill="#fff"/>'
                 f'<circle cx="{C}" cy="{C}" r="{ri:.1f}" fill="#1d1230"/>')
    if not favicon:
        parts.append('<g clip-path="url(#c)" stroke="#3a2a55" stroke-width=".7">'
                     + "".join(f'<path d="M{x},0V128"/>' for x in range(8, 128, 8))
                     + "".join(f'<path d="M0,{y}H128"/>' for y in range(8, 128, 8)) + '</g>')
    parts.append(berry(ri))
    parts.append(f'<g clip-path="url(#c)">{pulse(width)}</g>')
    size = 'width="128" height="128" ' if not favicon else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" {size}role="img" '
            f'aria-label="{TITLE}"><title>{TITLE}</title>{"".join(parts)}</svg>\n')


if __name__ == "__main__":
    for path, favicon in (("docs/logo.svg", False), ("assets/favicon.svg", True)):
        with open(os.path.join(ROOT, path), "w") as fh:
            fh.write(svg(favicon))
        print(path, os.path.getsize(os.path.join(ROOT, path)), "bytes")
