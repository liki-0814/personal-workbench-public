#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Delayed Conversion Completion Transformer — publication-quality architecture diagram.
Generates a 16:9 SVG (3840x2160 viewBox) + PDF + PNG preview.
"""

W, H = 3840, 2160

# ---------- palette ----------
C_GRAY   = "#EDEDED"   # raw inputs
C_BLUE   = "#DCEBF7"   # feature processing / transformer
C_YELLOW = "#FDF3D7"   # BCM
C_GREEN  = "#E1F1E4"   # VRMP
C_ORANGE = "#FDEBD7"   # Is Future Convert
C_PURPLE = "#EDE7F6"   # MOE
C_FINAL  = "#EAF1FB"   # final output fill
C_DARKBLUE = "#1F4E9C" # final output stroke
C_RED    = "#C0392B"
C_TEXT   = "#111111"
C_SUB    = "#444444"
C_HATCH  = "#9E9E9E"

FONT = "Helvetica, Arial, sans-serif"
MONO = "Helvetica, Arial, sans-serif"

svg = []

def esc(t):
    return (t.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))

def rect(x, y, w, h, fill, stroke=C_TEXT, sw=1.6, dash=None, rx=6):
    d = f' stroke-dasharray="{dash}"' if dash else ""
    svg.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"{d}/>')

def text(x, y, t, size=26, bold=False, fill=C_TEXT, anchor="middle", style=None):
    b = ' font-weight="bold"' if bold else ""
    a = f' text-anchor="{anchor}"'
    s = f' font-style="{style}"' if style else ""
    svg.append(f'<text x="{x}" y="{y}" font-family="{FONT}" font-size="{size}" fill="{fill}"{b}{a}{s}>{esc(t)}</text>')

def line(x1, y1, x2, y2, sw=1.4, stroke=C_TEXT, dash=None):
    d = f' stroke-dasharray="{dash}"' if dash else ""
    svg.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{stroke}" stroke-width="{sw}"{d}/>')

def arrow(x1, y1, x2, y2, dashed=False, color=C_TEXT, sw=2.2, marker=None):
    """Straight arrow; elbow supported via mid point list."""
    d = ' stroke-dasharray="10,8"' if dashed else ""
    m = marker or ("arrow-dash" if dashed else "arrow")
    svg.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{color}" stroke-width="{sw}"{d} marker-end="url(#{m})"/>')

def polyline(points, dashed=False, color=C_TEXT, sw=2.2, arrow_end=True):
    d = ' stroke-dasharray="10,8"' if dashed else ""
    m = f' marker-end="url(#{"arrow-dash" if dashed else "arrow"})"' if arrow_end else ""
    pts = " ".join(f"{p[0]},{p[1]}" for p in points)
    svg.append(f'<polyline points="{pts}" fill="none" stroke="{color}" stroke-width="{sw}"{d}{m}/>')

def module(x, y, w, h, title, fill, lines=None, title_size=28, line_size=23, sw=1.6, dash=None, stroke=C_TEXT):
    rect(x, y, w, h, fill, stroke=stroke, sw=sw, dash=dash)
    text(x + w/2, y + 42, title, size=title_size, bold=True)
    if lines:
        yy = y + 84
        for ln in lines:
            text(x + w/2, yy, ln, size=line_size, fill=C_SUB)
            yy += 38

# ---------- defs ----------
svg.append(f'''<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
<defs>
  <marker id="arrow" markerWidth="12" markerHeight="12" refX="10" refY="5" orient="auto" markerUnits="strokeWidth">
    <path d="M0,0 L10,5 L0,10 z" fill="{C_TEXT}"/>
  </marker>
  <marker id="arrow-dash" markerWidth="12" markerHeight="12" refX="10" refY="5" orient="auto" markerUnits="strokeWidth">
    <path d="M0,0 L10,5 L0,10 z" fill="{C_SUB}"/>
  </marker>
  <pattern id="hatch" width="12" height="12" patternTransform="rotate(45)" patternUnits="userSpaceOnUse">
    <rect width="12" height="12" fill="{C_GRAY}"/>
    <line x1="0" y1="0" x2="0" y2="12" stroke="{C_HATCH}" stroke-width="2"/>
  </pattern>
</defs>
<rect width="{W}" height="{H}" fill="#FFFFFF"/>
''')

# ---------- title ----------
text(W/2, 70, "Delayed Conversion Completion Transformer", size=52, bold=True)
line(640, 100, 3200, 100, sw=2.0)

# =========================================================
# 1. INPUT SECTION (left)
# =========================================================
IN_X = 90
IN_W = 560

# --- Group A: Static Categorical Features ---
gy = 170
module(IN_X, gy, IN_W, 430, "Static Categorical Features", C_GRAY)
feats_a = ["use_id", "bucket_type", "convert_type", "bucket_id", "industry1_id", "ad_slot_id"]
fy = gy + 100
for f in feats_a:
    rect(IN_X + 60, fy, 200, 44, "#FFFFFF", sw=1.2)
    text(IN_X + 160, fy + 30, f, size=22, fill=C_TEXT)
    fy += 54
# Embedding layer box
emb_y = gy + 455
rect(IN_X, emb_y, IN_W, 80, C_BLUE, sw=1.6)
text(IN_X + IN_W/2, emb_y + 50, "Embedding Layer", size=27, bold=True)
# arrow features -> embedding
arrow(IN_X + IN_W/2, gy + 430, IN_X + IN_W/2, emb_y - 4)

# --- Group B: Temporal Tokens ---
gy2 = emb_y + 130
module(IN_X, gy2, IN_W, 610, "96 × 15-min Temporal Tokens", C_GRAY)
# token row: 9 visual slots
tok_y = gy2 + 95
tok_w, tok_h, gap = 58, 44, 8
tx = IN_X + 28
labels = ["Token 1", "Token 2", "Token 3", "…", "Token k", "…", "Token k+1", "…", "Token 96"]
observed = [True, True, True, True, True, True, False, False, False]
for i, lab in enumerate(labels):
    fill = C_BLUE if observed[i] else "url(#hatch)"
    rect(tx, tok_y, tok_w if lab != "…" else 30, tok_h, fill, sw=1.2)
    if lab != "…":
        text(tx + tok_w/2, tok_y + 29, lab.replace("Token ", "T"), size=18)
    else:
        text(tx + 15, tok_y + 30, "…", size=20)
    tx += (tok_w if lab != "…" else 30) + gap
# brackets
br_y = tok_y + 62
line(IN_X + 28, br_y, IN_X + 28 + 3*66 + 8 + 30 + 8 + 58, br_y, sw=2.0)
text(IN_X + 200, br_y + 30, "Observed Prefix (1..k)", size=21, bold=True, fill="#2E6DA4")
hx = IN_X + 28 + 3*66 + 8 + 30 + 8 + 58 + 20
line(hx, br_y, IN_X + IN_W - 28, br_y, sw=2.0)
text(hx + 90, br_y + 30, "Future Masked (k+1..96)", size=21, bold=True, fill="#666666")
# feature list two columns
feat_b = ["ad_show", "clk_num", "sum_picvr", "charge", "origin_charge", "adjusted_price",
          "original_bid", "tcpa", "observed_conv", "click_segment", "current_segment", "age_segment"]
col1, col2 = feat_b[:6], feat_b[6:]
ly = gy2 + 210
for i, f in enumerate(col1):
    text(IN_X + 70, ly + i*44, "• " + f, size=21, anchor="start", fill=C_SUB)
for i, f in enumerate(col2):
    text(IN_X + 300, ly + i*44, "• " + f, size=21, anchor="start", fill=C_SUB)

# --- Group C: Historical Delay Curve ---
gy3 = gy2 + 650
module(IN_X, gy3, IN_W, 250, "Historical Delay Curve", C_GRAY)
rect(IN_X + 80, gy3 + 90, IN_W - 160, 60, "#FFFFFF", sw=1.2)
text(IN_X + IN_W/2, gy3 + 128, "[d1, d2, d3, …, d96]", size=25)
text(IN_X + IN_W/2, gy3 + 190, "F(age) = cumulative maturity", size=21, fill=C_SUB)
text(IN_X + IN_W/2, gy3 + 225, "1 − F(age) = remaining ratio", size=21, fill=C_SUB)

# --- Prior modules (between input and gating) ---
pr_x = IN_X + IN_W + 60
pr_w = 470
module(pr_x, 300, pr_w, 150, "pICVR Remaining Prior", C_GRAY,
       lines=["R_picvr = sum_picvr × (1 − F(age))"], line_size=22)
module(pr_x, 520, pr_w, 150, "Return Extrapolation", C_GRAY,
       lines=["R_return = observed_conv × (1 − F(age))", "                    / (F(age) + ε)"], line_size=22)

# =========================================================
# 2. GATING
# =========================================================
ga_x = pr_x + pr_w + 120
ga_w = 480
ga_y = 420
ga_h = 240
module(ga_x, ga_y, ga_w, ga_h, "TFT-style Variable Selection", C_BLUE, lines=["& Gating"], line_size=26)

# arrows: inputs -> gating (elbow polylines)
gate_in_x = ga_x
gate_mid_y = ga_y + ga_h/2
# A embedding
polyline([(IN_X + IN_W, emb_y + 40), (IN_X + IN_W + 30, emb_y + 40), (IN_X + IN_W + 30, gate_mid_y - 60), (gate_in_x - 4, gate_mid_y - 60)])
# B tokens
polyline([(IN_X + IN_W, gy2 + 200), (IN_X + IN_W + 30, gy2 + 200), (IN_X + IN_W + 30, gate_mid_y), (gate_in_x - 4, gate_mid_y)])
# C delay curve
polyline([(IN_X + IN_W, gy3 + 120), (IN_X + IN_W + 30, gy3 + 120), (IN_X + IN_W + 30, gate_mid_y + 60), (gate_in_x - 4, gate_mid_y + 60)])
# priors
arrow(pr_x + pr_w, 375, gate_in_x - 4, gate_mid_y - 30)
arrow(pr_x + pr_w, 595, gate_in_x - 4, gate_mid_y + 30)

# =========================================================
# 3. TRANSFORMER ENCODER
# =========================================================
te_x = ga_x + ga_w + 130
te_w = 620
te_y = 300
te_h = 560
module(te_x, te_y, te_w, te_h, "Transformer Encoder", C_BLUE, title_size=34)
specs = ["6 Encoder Layers", "d_model = 256", "8 Attention Heads", "FFN = 1024", "Pre-LN",
         "Bidirectional Attention", "within Observed Prefix"]
sy = te_y + 110
for s in specs:
    rect(te_x + 90, sy - 28, te_w - 180, 46, "#FFFFFF", sw=1.0)
    text(te_x + te_w/2, sy + 4, s, size=23)
    sy += 58

arrow(ga_x + ga_w, gate_mid_y, te_x - 4, te_y + te_h/2)

# outputs h1..hk
h_y = te_y + te_h + 60
hx0 = te_x + 40
for i, lab in enumerate(["h1", "h2", "h3", "…", "hk"]):
    cx = hx0 + i*110
    if lab != "…":
        svg.append(f'<circle cx="{cx}" cy="{h_y}" r="30" fill="#FFFFFF" stroke="{C_TEXT}" stroke-width="1.6"/>')
        text(cx, h_y + 9, lab, size=23)
    else:
        text(cx, h_y + 9, "…", size=26)
    # connect from encoder bottom
    arrow(cx, te_y + te_h, cx, h_y - 34)
text(te_x + te_w/2, h_y + 80, "One hidden representation for each clk_time segment", size=22, style="italic", fill=C_SUB)

# =========================================================
# 4. PREDICTION HEADS
# =========================================================
hd_x = te_x + te_w + 200
hd_w = 640

# Branch A: Is Future Convert (pale orange)
a_y = 200
a_h = 250
module(hd_x, a_y, hd_w, a_h, "Is Future Convert Head", C_ORANGE, title_size=30)
text(hd_x + hd_w/2, a_y + 110, "q_j = P(R_j > 0 | x)", size=26)
# sigmoid icon (simple S curve)
sx0, sy0 = hd_x + hd_w/2 - 90, a_y + 190
svg.append(f'<path d="M {sx0} {sy0+30} C {sx0+60} {sy0+30}, {sx0+120} {sy0-30}, {sx0+180} {sy0-30}" fill="none" stroke="{C_TEXT}" stroke-width="2.4"/>')
line(sx0 - 10, sy0 + 42, sx0 + 190, sy0 + 42, sw=1.2, stroke="#999999")
line(sx0 - 10, sy0 + 42, sx0 - 10, sy0 - 46, sw=1.2, stroke="#999999")
text(hd_x + hd_w/2, a_y + 240, "sigmoid output", size=19, fill=C_SUB)

# Branch B: BCM (pale yellow)
b_y = 520
b_h = 430
module(hd_x, b_y, hd_w, b_h, "BCM Ordinal Expert", C_YELLOW, title_size=30)
# 20 buckets in 2 rows of 10
bk_w, bk_h = 52, 40
for r in range(2):
    for c in range(10):
        bx = hd_x + 40 + c * (bk_w + 6)
        by = b_y + 100 + r * 54
        rect(bx, by, bk_w, bk_h, "#FFFFFF", sw=1.1)
        text(bx + bk_w/2, by + 27, f"B{r*10+c}", size=18)
byy = b_y + 240
for ln in ["Ordered cumulative probabilities", "Monotonic constraint", "Robust interval estimation"]:
    text(hd_x + hd_w/2, byy, ln, size=22, fill=C_SUB)
    byy += 42

# Branch C: VRMP (pale green)
c_y = 1020
c_h = 330
module(hd_x, c_y, hd_w, c_h, "VRMP Continuous Expert", C_GREEN, title_size=30)
rect(hd_x + 150, c_y + 90, hd_w - 300, 60, "#FFFFFF", sw=1.2)
text(hd_x + hd_w/2, c_y + 128, "DNN Residual Correction", size=24)
text(hd_x + hd_w/2, c_y + 215, "z = log(1 + R_true) − log(1 + R_picvr)", size=24)
text(hd_x + hd_w/2, c_y + 285, "Continuous in-bucket correction", size=22, style="italic", fill=C_SUB)

# arrows: h_j -> heads (bus)
bus_x = te_x + te_w + 90
polyline([(te_x + te_w, h_y), (bus_x, h_y)], arrow_end=False)
for ty in [a_y + a_h/2, b_y + b_h/2, c_y + c_h/2]:
    polyline([(bus_x, h_y), (bus_x, ty), (hd_x - 4, ty)])
text(bus_x - 10, h_y - 40, "h_j", size=24, bold=True, anchor="end")

# =========================================================
# 5. MOE FUSION
# =========================================================
mo_x = hd_x + hd_w + 140
mo_w = 640
mo_y = 640
mo_h = 300
module(mo_x, mo_y, mo_w, mo_h, "MOE Gate", C_PURPLE, title_size=30)
text(mo_x + mo_w/2, mo_y + 110, "g_j = sigmoid(Gate(h_j))", size=25)
text(mo_x + mo_w/2, mo_y + 180, "R_positive = g_j × R_BCM", size=24)
text(mo_x + mo_w/2, mo_y + 225, "             + (1 − g_j) × R_VRMP", size=24)

arrow(hd_x + hd_w, b_y + b_h/2, mo_x - 4, mo_y + 90)          # BCM -> MOE
arrow(hd_x + hd_w, c_y + c_h/2, mo_x - 4, mo_y + 230)         # VRMP -> MOE
polyline([(bus_x, c_y + c_h/2 + 120), (bus_x, mo_y + 160), (mo_x - 4, mo_y + 160)], arrow_end=False)  # gate input h_j (visual hint)
arrow(bus_x, mo_y + 160, mo_x - 4, mo_y + 160)

# merge node: R_pred = q * R_positive
mg_y = mo_y + mo_h + 90
mg_h = 130
rect(mo_x, mg_y, mo_w, mg_h, C_PURPLE, sw=1.6)
text(mo_x + mo_w/2, mg_y + 55, "Merge", size=25, bold=True)
text(mo_x + mo_w/2, mg_y + 100, "R_pred_j = q_j × R_positive", size=25)
arrow(mo_x + mo_w/2, mo_y + mo_h, mo_x + mo_w/2, mg_y - 4)
# Is Future Convert -> Merge
polyline([(hd_x + hd_w, a_y + a_h/2), (mo_x + mo_w/2 + 160, a_y + a_h/2), (mo_x + mo_w/2 + 160, mg_y + mg_h/2), (mo_x + mo_w, mg_y + mg_h/2)])
text(mo_x + mo_w/2 + 160, a_y + a_h/2 - 20, "q_j", size=22, bold=True)

# =========================================================
# 6. FINAL OUTPUT
# =========================================================
fn_x = mo_x + mo_w + 160
fn_w = 620

# Remaining conversion box
rc_y = 500
rc_h = 130
module(fn_x, rc_y, fn_w, rc_h, "Predicted Remaining", C_BLUE, lines=["Conversion per clk_time"], title_size=26, line_size=24)
arrow(mo_x + mo_w, mg_y + mg_h/2, fn_x - 4, rc_y + rc_h/2)
text((mo_x + mo_w + fn_x)/2, mg_y + mg_h/2 - 25, "R_pred_j", size=22, bold=True)

# Observed conversion input
oc_y = 720
rect(fn_x + 40, oc_y, fn_w - 80, 90, C_GRAY, sw=1.6)
text(fn_x + fn_w/2, oc_y + 55, "Observed Conversion O_j", size=26, bold=True)

# addition node
add_cx, add_cy = fn_x + fn_w/2, 920
svg.append(f'<circle cx="{add_cx}" cy="{add_cy}" r="46" fill="#FFFFFF" stroke="{C_TEXT}" stroke-width="2.2"/>')
line(add_cx - 20, add_cy, add_cx + 20, add_cy, sw=2.6)
line(add_cx, add_cy - 20, add_cx, add_cy + 20, sw=2.6)
arrow(fn_x + fn_w/2, rc_y + rc_h, add_cx, add_cy - 50)
arrow(fn_x + fn_w/2, oc_y + 90, add_cx, add_cy + 50)
text(add_cx + 130, add_cy + 8, "N_pred_j = O_j + R_pred_j", size=25, anchor="start")

# summation node
sum_cy = 1150
svg.append(f'<circle cx="{add_cx}" cy="{sum_cy}" r="60" fill="#FFFFFF" stroke="{C_TEXT}" stroke-width="2.4"/>')
text(add_cx, sum_cy + 18, "Σ", size=52, bold=True)
arrow(add_cx, add_cy + 50, add_cx, sum_cy - 66)
text(add_cx + 100, sum_cy + 8, "over all observed clk_time segments", size=22, anchor="start", fill=C_SUB)

# final output box
fo_y = 1340
fo_h = 190
rect(fn_x - 30, fo_y, fn_w + 60, fo_h, C_FINAL, stroke=C_DARKBLUE, sw=5)
text(fn_x + fn_w/2, fo_y + 70, "Predicted Final 24h", size=34, bold=True, fill=C_DARKBLUE)
text(fn_x + fn_w/2, fo_y + 115, "Conversion Count", size=34, bold=True, fill=C_DARKBLUE)
text(fn_x + fn_w/2, fo_y + 165, "C_pred_24h = Σ_j (O_j + R_pred_j)", size=27, fill=C_DARKBLUE)
arrow(add_cx, sum_cy + 66, add_cx, fo_y - 6)

# =========================================================
# 7. DETERMINISTIC GMV (dashed)
# =========================================================
gm_y = 1660
gm_h = 190
rect(fn_x - 30, gm_y, fn_w + 60, gm_h, "#FFFFFF", stroke=C_SUB, sw=2.2, dash="12,9")
text(fn_x + fn_w/2, gm_y + 60, "Deterministic Post-processing", size=28, bold=True, fill=C_SUB)
text(fn_x + fn_w/2, gm_y + 115, "GMV_pred = Σ_j tcpa_j × N_pred_j", size=27, fill=C_SUB)
text(fn_x + fn_w/2, gm_y + 165, "No Separate GMV Model", size=26, bold=True, fill=C_RED)
arrow(add_cx, fo_y + fo_h, add_cx, gm_y - 6, dashed=True)

# ---------- legend ----------
lg_x, lg_y = 90, 1990
text(lg_x, lg_y, "Legend:", size=23, bold=True, anchor="start")
legend = [("Raw inputs", C_GRAY), ("Feature / Transformer", C_BLUE), ("BCM", C_YELLOW),
          ("VRMP", C_GREEN), ("Is Future Convert", C_ORANGE), ("MOE", C_PURPLE)]
lx = lg_x + 120
for name, col in legend:
    rect(lx, lg_y - 24, 34, 26, col, sw=1.2)
    text(lx + 44, lg_y - 4, name, size=20, anchor="start", fill=C_SUB)
    lx += 44 + len(name) * 12 + 50
rect(lx, lg_y - 24, 34, 26, C_FINAL, stroke=C_DARKBLUE, sw=3)
text(lx + 44, lg_y - 4, "Final output", size=20, anchor="start", fill=C_SUB)
lx += 44 + 12 * 12 + 50
line(lx + 10, lg_y - 11, lx + 70, lg_y - 11, sw=2.2)
text(lx + 80, lg_y - 4, "Learned flow", size=20, anchor="start", fill=C_SUB)
lx += 80 + 12 * 11 + 40
line(lx + 10, lg_y - 11, lx + 70, lg_y - 11, sw=2.2, dash="10,8")
text(lx + 80, lg_y - 4, "Deterministic", size=20, anchor="start", fill=C_SUB)

svg.append("</svg>")

out = "".join(svg)
with open("dcc_transformer.svg", "w", encoding="utf-8") as f:
    f.write(out)
print("SVG written:", len(out), "bytes")
