#!/usr/bin/env python3
"""Build assets/gpudb.json, the reference spec database of gpu-r.

Every record joins two independent kinds of source:

  * PCI device ID (+ revision) -> product name, from the vendors themselves:
      - AMD:    libdrm data/amdgpu.ids (maintained by AMD; has revision-exact
                names, e.g. 73BF rev C1 = RX 6800 XT) and pci.ids for
                pre-GCN chips that amdgpu.ids does not cover
      - NVIDIA: "Supported NVIDIA GPU Products" appendix of the Linux driver
                README (390.xx for Fermi, 470.xx for Kepler, 580.xx for the rest)
  * product name -> factory specs, from the spec tables of the Wikipedia
    "List of AMD/Nvidia graphics processing units" articles, pinned to a
    revision id so the build is reproducible.

Usage:
  python3 -m venv venv && venv/bin/pip install pandas lxml
  venv/bin/python tools/build_gpudb.py CACHE_DIR assets/gpudb.json

Sources are downloaded into CACHE_DIR on first use. Unmatched SKUs and IDs
are listed on stderr so gaps are visible, never silently filled in.
"""

import io
import json
import re
import sys
import urllib.request
from pathlib import Path

import pandas as pd
from lxml import html

WIKI_AMD_REV = 1385626444  # 2026-11-14
WIKI_NV_REV = 1385652772  # 2026-11-14
NV_README = ["390.157", "470.256.02", "580.95.05"]

SOURCES = {
    "amdgpu.ids": "https://gitlab.freedesktop.org/mesa/drm/-/raw/main/data/amdgpu.ids",
    "pci.ids": "https://pci-ids.ucw.cz/v2.2/pci.ids",
    "wiki-amd.html": f"https://en.wikipedia.org/w/index.php?oldid={WIKI_AMD_REV}&action=render",
    "wiki-nv.html": f"https://en.wikipedia.org/w/index.php?oldid={WIKI_NV_REV}&action=render",
    **{f"nv-{v}.html": f"https://download.nvidia.com/XFree86/Linux-x86_64/{v}/README/supportedchips.html" for v in NV_README},
}

# Desktop series covered: Radeon HD 4000 .. RX 9000 + Radeon AI Pro R9000,
# GeForce 400 .. RTX 50 (incl. Titan / Titan V).
AMD_SECTIONS = [
    "Radeon HD 4000 series", "Radeon HD 5000 series", "Radeon HD 6000 series", "Radeon HD 7000 series",
    "Radeon HD 8000 series", "Radeon 200 series", "Radeon 300 series", "Radeon 400 series", "Radeon 500 series",
    "Radeon RX Vega series", "Radeon VII series", "Radeon RX 5000 series", "Radeon RX 6000 series",
    "Radeon RX 7000 series", "Radeon RX 9000 series", "Radeon Pro R9000 series",
]
NV_SECTIONS = [
    "GeForce 400 series", "GeForce 500 series", "GeForce 600 series", "GeForce 700 series", "GeForce 900 series",
    "GeForce 10 series", "Volta series", "GeForce GTX 16 series", "GeForce RTX 20 series", "GeForce RTX 30 series",
    "GeForce RTX 40 series", "GeForce RTX 50 series",
]

# Facts that the spec tables do not carry, per chip.
AMD_GC_IP = {
    "Navi 10": (10, 1, 10), "Navi 12": (10, 1, 2), "Navi 14": (10, 1, 1),
    "Navi 21": (10, 3, 0), "Navi 22": (10, 3, 2), "Navi 23": (10, 3, 4), "Navi 24": (10, 3, 5),
    "Navi 31": (11, 0, 0), "Navi 32": (11, 0, 3), "Navi 33": (11, 0, 2),
    "Navi 44": (12, 0, 0), "Navi 48": (12, 0, 1),
}
# NVIDIA CUDA compute capability and process by chip prefix (CUDA programming guide).
NV_ARCH = [
    # prefix, compute capability, cores per SM, process
    ("GF100", (2, 0), 32, "40 nm"), ("GF110", (2, 0), 32, "40 nm"), ("GF1", (2, 1), 48, "40 nm"),
    ("GK110", (3, 5), 192, "28 nm"), ("GK208", (3, 5), 192, "28 nm"), ("GK1", (3, 0), 192, "28 nm"),
    ("GM107", (5, 0), 128, "28 nm"), ("GM108", (5, 0), 128, "28 nm"), ("GM2", (5, 2), 128, "28 nm"),
    ("GP100", (6, 0), 64, "16 nm"), ("GP107", (6, 1), 128, "14 nm"), ("GP108", (6, 1), 128, "14 nm"),
    ("GP1", (6, 1), 128, "16 nm"),
    ("GV100", (7, 0), 64, "12 nm"), ("TU1", (7, 5), 64, "12 nm"),
    ("GA100", (8, 0), 64, "7 nm"), ("GA1", (8, 6), 128, "8 nm"),
    ("AD1", (8, 9), 128, "TSMC 4N"), ("GB2", (12, 0), 128, "TSMC 4N"),
]


def fetch(cache: Path):
    cache.mkdir(parents=True, exist_ok=True)
    for name, url in SOURCES.items():
        p = cache / name
        if not p.exists():
            print(f"downloading {url}", file=sys.stderr)
            req = urllib.request.Request(url, headers={"User-Agent": "gpu-r-dbbuild/1.0"})
            p.write_bytes(urllib.request.urlopen(req, timeout=60).read())


# ----------------------------------------------------------------- names

def clean(s) -> str:
    s = str(s).replace("\xa0", " ").replace(" ", " ")
    s = re.sub(r"\[[^\]]*\]", "", s)
    return re.sub(r"\s+", " ", s).strip()


def norm(name: str) -> str:
    s = clean(name).lower()
    s = re.sub(r"\([^)]*\)", " ", s)
    s = re.sub(r"\b(amd|ati|nvidia|graphics|edition)\b", " ", s)
    s = s.replace("radeon ai pro", "radeon pro")
    s = re.sub(r"(\d)\s*gb\b", r"\1gb", s)
    s = re.sub(r"[^a-z0-9 ]+", " ", s)
    return re.sub(r"\s+", " ", s).strip()


def expand_pciids(label: str) -> list[str]:
    """'Tahiti XT [Radeon HD 7970/8970 OEM / R9 280X]' -> names."""
    m = re.search(r"\[([^\]]+)\]", label)
    if not m:
        return []
    out = []
    parts = [p.strip() for p in m.group(1).split(" / ")]
    brand = ""
    for i, part in enumerate(parts):
        words = part.split()
        if not words:
            continue
        if i == 0:
            k = next((j for j, w in enumerate(words) if re.search(r"\d", w)), len(words))
            brand = " ".join(words[:k])
        elif re.match(r"(?i)^(radeon|firepro|geforce)", part):
            pass
        elif re.match(r"(?i)^(r[579]|rx|hd)\b", part):
            part = "Radeon " + part
        else:
            part = f"{brand} {part}"
        words = part.split()
        k = next((j for j, w in enumerate(words) if "/" in w), None)
        if k is None:
            out.append(part)
            continue
        prefix, rest = words[:k], words[k + 1:]
        alts = words[k].split("/")
        for j, a in enumerate(alts):
            tail = rest if j == len(alts) - 1 else []
            out.append(" ".join(prefix + [a] + tail))
    return out


# ----------------------------------------------------------------- ID sources

def amd_ids(cache: Path):
    """(device, rev|None, name, source)"""
    out = []
    covered = set()
    for line in (cache / "amdgpu.ids").read_text().splitlines():
        m = re.match(r"^([0-9A-Fa-f]{4}),\s*([0-9A-Fa-f]{2}),\s*(.+)$", line)
        if m:
            dev, rev = int(m.group(1), 16), int(m.group(2), 16)
            for n in m.group(3).split(" / "):
                out.append((dev, rev, n.strip(), "libdrm amdgpu.ids"))
            covered.add(dev)
    for dev, label in pciids_devices(cache, "1002"):
        for n in expand_pciids(label):
            out.append((dev, None, n, "pci.ids"))
    return out, covered


def pciids_devices(cache: Path, vendor: str):
    cur = None
    for line in (cache / "pci.ids").read_text(errors="replace").splitlines():
        if not line or line.startswith("#"):
            continue
        if not line.startswith("\t"):
            cur = line[:4].lower()
            continue
        if cur == vendor and not line.startswith("\t\t"):
            m = re.match(r"^\t([0-9a-f]{4})\s+(.+)$", line)
            if m:
                yield int(m.group(1), 16), m.group(2)


def nv_ids(cache: Path):
    out = []
    for v in NV_README:
        doc = html.fromstring((cache / f"nv-{v}.html").read_text(errors="replace"))
        for tr in doc.iter("tr"):
            tds = [clean(td.text_content()) for td in tr.findall("td")]
            if len(tds) >= 2 and re.fullmatch(r"[0-9A-Fa-f]{4}", tds[1].split()[0] if tds[1] else ""):
                out.append((int(tds[1].split()[0], 16), None, tds[0], f"NVIDIA driver {v} README"))
    return out


# ----------------------------------------------------------------- spec tables

def tables_by_section(path: Path, wanted: list[str]):
    doc = html.fromstring(path.read_text(encoding="utf-8"))
    heads = []
    for el in doc.iter():
        if el.tag in ("h2", "h3", "h4"):
            lvl = int(el.tag[1])
            heads = heads[: lvl - 2] + [clean(el.text_content())]
        elif el.tag == "table" and "wikitable" in (el.get("class") or ""):
            sec = heads[-1] if heads else ""
            if len(heads) >= 2 and heads[0] == "Desktop GPUs" or "Workstation" in (heads[0] if heads else ""):
                if sec in wanted:
                    try:
                        df = pd.read_html(io.StringIO(html.tostring(el, encoding="unicode")))[0]
                    except ValueError:
                        continue
                    yield sec, df


def col(df, *keys, exclude=()):
    for c in df.columns:
        t = " | ".join(str(x) for x in (c if isinstance(c, tuple) else (c,))).lower().replace("- ", "").replace("\xa0", " ")
        if all(k in t for k in keys) and not any(e in t for e in exclude):
            return c
    return None


def nums(s) -> list[float]:
    s = clean(s).replace(",", "")
    return [float(x) for x in re.findall(r"\d+(?:\.\d+)?", s)]


def parse_row(df, r, vendor):
    get = lambda c: clean(df.at[r, c]) if c is not None else ""
    model = get(df.columns[0])
    if not model or model.lower().startswith(("model", "nan")) or not re.search(r"\d", model):
        return None
    rec = {"name": re.sub(r"\s*\([^)]*\)\s*$", "", model).strip(), "raw_name": model}
    cm = re.search(r"\(([^)]*)\)", model)
    cn = col(df, "code name", exclude=("model",))
    code = get(cn) or (cm.group(1) if cm else "")
    rec["chip"] = code
    rec["release"] = ""
    rel = get(col(df, "launch") or col(df, "release"))
    m = re.search(r"([A-Z][a-z]{2,8}\.? \d{1,2}, \d{4}|[A-Z][a-z]{2,8}\.? \d{4}|\d{4})", rel)
    rec["release"] = m.group(1) if m else ""

    # core config "shaders:TMUs:ROPs[:...]" and optional "NN CU"
    cfg = get(col(df, "config"))
    m = re.match(r"^\s*(\d+)\s*:\s*(\d+)\s*:\s*(\d+)", cfg.replace(",", ""))
    if not m:
        return None
    rec["shaders"], rec["tmus"], rec["rops"] = int(m.group(1)), int(m.group(2)), int(m.group(3))
    cu = re.search(r"(\d+)\s*CU", cfg)
    rec["units"] = int(cu.group(1)) if cu else None
    rec["cfg_raw"] = cfg

    # clocks: highest core/boost value
    cc = col(df, "clock", "core") or col(df, "clock", "base") or col(df, "core", "clock")
    rec["boost_mhz"] = int(max(nums(get(cc))[:4] or [0]))

    # fillrates as published, used only to cross-check the other columns
    rec["px_fill"] = max(nums(get(col(df, "fillrate", "pixel")))[:4] or [0])
    rec["tx_fill"] = max(nums(get(col(df, "fillrate", "texture")))[:4] or [0])

    # memory
    mt = get(col(df, "bus type") or col(df, "dram type") or col(df, "memory", "type"))
    t = re.findall(r"(GDDR\d[X]?|HBM\d?e?|DDR\d|GDDR\d)", mt.upper())
    rec["mem"] = t[0] if t else ""
    bwc = col(df, "bus width") or col(df, "bus type & width")
    bwv = [int(x) for x in re.findall(r"(\d+)\s*-?\s*bit", get(bwc))] or [int(x) for x in nums(get(bwc)) if x >= 32]
    if not bwv:
        bwv = [int(x) for x in re.findall(r"(\d+)\s*-?\s*bit", mt)]
    rec["bus_width"] = max(bwv) if bwv else 0
    szc = col(df, "memory", "size") or col(df, "size")
    szt = get(szc)
    hdr = " ".join(str(x) for x in (szc if isinstance(szc, tuple) else (szc,))).lower() if szc is not None else ""
    sizes = []
    for v, u in re.findall(r"(\d+(?:\.\d+)?)\s*(GB|MB|GiB|MiB)?", szt):
        v = float(v)
        unit = (u or ("mb" if "(mb)" in hdr else "gb" if "(gb)" in hdr else ("mb" if v >= 128 else "gb"))).lower()
        mb = int(v * 1024) if unit.startswith("g") else int(v)
        if 128 <= mb <= 262144:
            sizes.append(mb)
    rec["mem_sizes_mb"] = sorted(set(sizes))
    bw = nums(get(col(df, "bandwidth", exclude=("infinity", "cache"))))
    rec["mem_gbps"] = round(max(bw) * 8 / rec["bus_width"], 2) if bw and rec["bus_width"] else 0.0

    # process, transistors, die
    fc = col(df, "fab", exclude=("processing",))
    fab = get(fc)
    rec["process"] = ""
    if fab:
        hdr = " ".join(str(x) for x in (fc if isinstance(fc, tuple) else (fc,))).lower()
        m = (re.search(r"(TSMC|GlobalFoundries|GloFo|GF|Samsung|UMC)\s*(N\d+\w*|\d+\s*nm|\d+\s*[A-Z]+\w*|\d+)", fab)
             or re.search(r"(\d+)\s*nm", fab))
        if m and m.lastindex == 2:
            node = m.group(2)
            rec["process"] = f"{m.group(1)} {node} nm" if node.isdigit() else f"{m.group(1)} {node}"
        elif m:
            rec["process"] = f"{m.group(1)} nm"
        elif "(nm)" in hdr and nums(fab):
            rec["process"] = f"{int(nums(fab)[0])} nm"
        rec["process"] = rec["process"].replace("GlobalFoundries", "GF").replace("GloFo", "GF")
    trc = col(df, "transistor")
    trt = get(trc)
    hdr = " ".join(str(x) for x in (trc if isinstance(trc, tuple) else (trc,))).lower() if trc is not None else ""
    tr_m = 0
    m = re.search(r"(\d+(?:\.\d+)?)\s*(?:×\s*10\s*\^?(\d+)|billion|million)?", trt)
    if m and m.group(1):
        v = float(m.group(1))
        if m.group(2):
            tr_m = v * 10 ** int(m.group(2)) / 1e6
        elif "billion" in trt.lower() or "(billion)" in hdr:
            tr_m = v * 1000
        else:
            tr_m = v
    rec["transistors_m"] = int(round(tr_m))
    dc = col(df, "die size")
    die = get(dc)
    m = re.search(r"(\d+(?:\.\d+)?)\s*mm", die)
    if m:
        rec["die_mm2"] = int(round(float(m.group(1))))
    elif dc is not None and dc != trc and nums(die):
        rec["die_mm2"] = int(round(nums(die)[0]))
    else:
        rec["die_mm2"] = 0
    return rec


def chip_of(rec, vendor):
    c = clean(rec["chip"])
    if vendor == "nv":
        m = re.match(r"(G[A-Z]\d{3}|AD\d{3}|TU\d{3}|GB\d{3})", c.upper())
        return m.group(1) if m else c
    m = re.match(r"(Navi \d+|Vega \d+|Polaris \d+|Fiji|Tonga|Hawaii|Tahiti|Pitcairn|Cape Verde|Bonaire|Oland|"
                 r"Caicos|Cedar|Turks|Juniper|Redwood|Cypress|Hemlock|Barts|Cayman|Antilles|Antigua|Grenada|Trinidad|"
                 r"Tobago|Curacao|Ellesmere|Baffin|Lexa|Banks|Mars|Cape|Iceland|RV\d+|R\d+)", c, re.I)
    return m.group(1) if m else c


# Variant suffixes that share the base model's device ID ("OEM", "Rev. 2", "16 GB", ...).
QUALIFIER = re.compile(r"\s+(oem( rebrand)?|rebrand|rev \d+|mac|v2( e s)?|e s|low power|boost|ghz|eyefinity|"
                       r"\d+gb|gddr\dx?|core \d+|lhr)$")


def consistency(rec):
    """Cross-check the published numbers against each other (catches table typos)."""
    out = []
    clk = rec["boost_mhz"]
    near = lambda a, b, tol=0.06: b > 0 and abs(a - b) / b <= tol
    if rec["px_fill"] and clk:
        calc = rec["rops"] * clk / 1000
        out.append((f"ROPs x clock = {calc:.1f} GP/s vs published {rec['px_fill']:.1f}", near(calc, rec["px_fill"])))
    if rec["tx_fill"] and clk:
        calc = rec["tmus"] * clk / 1000
        out.append((f"TMUs x clock = {calc:.1f} GT/s vs published {rec['tx_fill']:.1f}", near(calc, rec["tx_fill"])))
    if rec["units"] and rec["shaders"]:
        out.append(("shaders divisible by units", rec["shaders"] % rec["units"] == 0))
    ranges = {"GDDR3": (1.0, 2.8), "GDDR4": (2.0, 3.0), "GDDR5": (3.0, 9.0), "GDDR5X": (10.0, 12.0),
              "GDDR6": (12.0, 20.5), "GDDR6X": (19.0, 24.0), "GDDR7": (27.0, 34.0)}
    if rec["mem_gbps"] and rec["mem"] in ranges:
        lo, hi = ranges[rec["mem"]]
        out.append((f"{rec['mem']} at {rec['mem_gbps']} Gbps is within the {rec['mem']} range {lo}-{hi}", lo <= rec["mem_gbps"] <= hi))
    return out


def build(cache: Path):
    fetch(cache)
    out = []
    log = []
    for vendor, path, sections, idlist, vid in (
        ("amd", cache / "wiki-amd.html", AMD_SECTIONS, None, 0x1002),
        ("nv", cache / "wiki-nv.html", NV_SECTIONS, None, 0x10DE),
    ):
        if vendor == "amd":
            ids, covered = amd_ids(cache)
        else:
            ids, covered = nv_ids(cache), set()
        dev_chip, mobile = {}, set()
        for dev, label in pciids_devices(cache, "10de" if vendor == "nv" else "1002"):
            if re.search(r"(?i)mobile|max-q|laptop|\bM\b", label) or re.match(r"G\w+\d+[A-Z]*M\b", label):
                mobile.add(dev)
            m = re.match(r"(G[A-Z]\d{3}|AD\d{3}|TU\d{3}|GB\d{3})", label)
            if m:
                dev_chip[dev] = m.group(1)
        by_name: dict[str, list] = {}
        for dev, rev, n, src in ids:
            by_name.setdefault(norm(n), []).append((dev, rev, src, n))
        recs = []
        for sec, df in tables_by_section(path, sections):
            df = df.reset_index(drop=True)
            for r in range(len(df)):
                rec = parse_row(df, r, vendor)
                if rec:
                    rec["series"] = sec
                    recs.append(rec)
        # de-duplicate rows repeated by rowspans
        seen, uniq = set(), []
        for rec in recs:
            k = (norm(rec["name"]), rec["chip"], rec["mem"], rec["shaders"], rec["bus_width"], tuple(rec["mem_sizes_mb"]))
            if k not in seen:
                seen.add(k)
                uniq.append(rec)
        for rec in uniq:
            rec["chip"] = chip_of(rec, vendor)
            if vendor == "nv":
                arch = next((a for a in NV_ARCH if rec["chip"].upper().startswith(a[0])), None)
                if arch:
                    rec["cuda_cc"] = list(arch[1])
                    rec["units"] = rec["shaders"] // arch[2] if arch[2] else None
                    rec["process"] = rec["process"] or arch[3]
            else:
                if rec["units"] is None:
                    vliw = 80 if re.match(r"(RV|R7|Cedar|Redwood|Juniper|Cypress|Hemlock|Caicos|Turks|Barts|Antilles)", rec["chip"]) else 64
                    rec["units"] = rec["shaders"] // vliw
                gc = AMD_GC_IP.get(rec["chip"])
                rec["gc_ip"] = list(gc) if gc else None
            n = norm(rec["name"])
            hits = list(by_name.get(n, []))
            for mb in rec["mem_sizes_mb"]:
                if not hits and mb >= 1024:
                    hits = list(by_name.get(f"{n} {mb // 1024}gb", []))
            alias = n
            while not hits:
                a2 = QUALIFIER.sub("", alias).strip()
                if a2 == alias or not a2:
                    break
                alias = a2
                hits = list(by_name.get(alias, []))
            if vendor == "nv":
                # the README names one SKU per ID; keep only IDs of the same chip (pci.ids codename)
                hits = [h for h in hits if not dev_chip.get(h[0]) or dev_chip[h[0]] == rec["chip"].upper()]
            # desktop database: drop notebook IDs that share a marketing name
            hits = [h for h in hits if h[0] not in mobile and not re.search(r"(?i)laptop|mobile|max-q", h[3])]
            if not hits:
                # "RX 580 Series" style names in amdgpu.ids
                for k, v in by_name.items():
                    if k.endswith(" series") and n.startswith(k[: -len(" series")]) and (len(n) == len(k) - 7 or n[len(k) - 7] == " "):
                        hits += v
            ids_out = sorted({(d, rv) for d, rv, src, _ in hits if not (rv is None and d in covered and any(x[0] == d and x[1] is not None for x in hits))},
                             key=lambda x: (x[0], -1 if x[1] is None else x[1]))
            rec["ids"] = [[d, rv] for d, rv in ids_out]
            rec["id_sources"] = sorted({src for _, _, src, _ in hits})
            rec["vendor"] = vid
            rec["spec_source"] = f"Wikipedia: List of {'AMD' if vendor == 'amd' else 'Nvidia'} graphics processing units, oldid {WIKI_AMD_REV if vendor == 'amd' else WIKI_NV_REV}, section '{rec['series']}'"
            if not rec["ids"]:
                log.append(f"[{vendor}] no device ID for {rec['raw_name']}")
            out.append(rec)
        # chip -> full die units (largest config seen on that chip)
        full = {}
        for rec in out:
            if rec["vendor"] == vid and rec.get("units"):
                full[rec["chip"]] = max(full.get(rec["chip"], 0), rec["units"])
        for rec in out:
            if rec["vendor"] == vid:
                rec["units_full_die"] = full.get(rec["chip"], rec.get("units") or 0)
    for rec in out:
        rec["checks"] = consistency(rec)
        rec["verified"] = all(ok for _, ok in rec["checks"])
        if not rec["verified"]:
            log.append(f"[check] {rec['name']}: " + "; ".join(t for t, ok in rec["checks"] if not ok))
    for rec in out:
        for k in ("raw_name", "cfg_raw", "series", "px_fill", "tx_fill"):
            rec.pop(k, None)
        rec.setdefault("gc_ip", None)
        rec.setdefault("cuda_cc", None)
        rec["units"] = rec.get("units") or 0
    return out, log


def main():
    cache, dst = Path(sys.argv[1]), Path(sys.argv[2])
    db, log = build(cache)
    for l in log:
        print(l, file=sys.stderr)
    with_ids = [r for r in db if r["ids"]]
    print(f"{len(db)} SKUs parsed, {len(with_ids)} with device IDs, "
          f"{sum(len(r['ids']) for r in with_ids)} ID entries", file=sys.stderr)
    meta = {
        "generated_by": "tools/build_gpudb.py",
        "sources": {
            "wikipedia_amd_oldid": WIKI_AMD_REV,
            "wikipedia_nvidia_oldid": WIKI_NV_REV,
            "nvidia_readme": NV_README,
            "amdgpu_ids": "libdrm main (data/amdgpu.ids)",
            "pci_ids": next((l.split(":", 1)[1].strip() for l in (cache / "pci.ids").read_text(errors="replace").splitlines()[:20] if "Version" in l), "?"),
        },
    }
    for r in with_ids:
        r["checks"] = [("ok: " if ok else "FAIL: ") + t for t, ok in r["checks"]]
    dst.write_text(json.dumps({"meta": meta, "gpus": with_ids}, ensure_ascii=False, separators=(",", ":")))


if __name__ == "__main__":
    main()
