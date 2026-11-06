#!/usr/bin/env python3
"""Integration checker for sparrow_engine (no Rust toolchain needed).
Usage: python tools/integration_check.py [crate_dir] [--fix-safe]
Writes INTEGRATION_REPORT.md. --fix-safe: Cargo deps, lib.rs/mod.rs module lines, child `mod` lines.
"""
import re, sys, shutil
from pathlib import Path

args = [a for a in sys.argv[1:] if not a.startswith("--")]
FIX = "--fix-safe" in sys.argv
root = Path(args[0] if args else ".").resolve()
src = root / "src"
rep, applied = [], []

def read(p): return p.read_text(encoding="utf-8", errors="replace")
def strip(t):
    t = re.sub(r"/\*.*?\*/", "", t, flags=re.S)
    t = re.sub(r"//[^\n]*", "", t)
    return re.sub(r'"(?:\\.|[^"\\])*"', '""', t)
def backup(p):
    b = p.with_suffix(p.suffix + ".bak")
    if not b.exists(): shutil.copy(p, b)

files = {p: read(p) for p in src.rglob("*.rs")}
code = {p: strip(t) for p, t in files.items()}
allcode = "\n".join(code.values())
defs = {p: re.sub(r"//[^\n]*", "", re.sub(r"/\*.*?\*/", "", tx, flags=re.S)) for p, tx in files.items()}  # strings kept: char literals like '"' break string stripping

def declared(modfile, name):
    return re.search(rf"\bmod\s+{name}\b", code.get(modfile, "")) is not None
def add_mod(modfile, line):
    backup(modfile)
    modfile.write_text(read(modfile).rstrip("\n") + "\n" + line + "\n", encoding="utf-8")
    applied.append(f"{modfile.relative_to(root)}: + `{line}`")

# ---- A. undeclared modules
lib = src / "lib.rs"
sec = ["## A. Undeclared modules"]
for p in sorted(src.glob("*.rs")):
    n = p.stem
    if n in ("lib", "main"): continue
    if not declared(lib, n):
        sec.append(f"- `src/{n}.rs` not in lib.rs")
        if FIX: add_mod(lib, f"pub mod {n};")
for d in sorted(x for x in src.iterdir() if x.is_dir()):
    mf = d / "mod.rs"
    if mf.exists():
        for p in sorted(d.glob("*.rs")):
            if p.stem != "mod" and not declared(mf, p.stem):
                sec.append(f"- `src/{d.name}/{p.stem}.rs` not in {d.name}/mod.rs")
                if FIX: add_mod(mf, f"pub mod {p.stem};")
    else:
        parent = src / f"{d.name}.rs"
        for p in sorted(d.glob("*.rs")):
            if parent.exists() and not declared(parent, p.stem):
                sec.append(f"- `src/{d.name}/{p.stem}.rs` not declared in `{d.name}.rs` (needs `mod {p.stem};`)")
                if FIX:
                    t = read(parent); backup(parent)
                    m = list(re.finditer(r"^use [^\n]+;[ \t]*$", t, flags=re.M))
                    pos = m[-1].end() if m else 0
                    parent.write_text(t[:pos] + f"\nmod {p.stem};" + t[pos:], encoding="utf-8")
                    applied.append(f"src/{d.name}.rs: + `mod {p.stem};`")
if len(sec) == 1: sec.append("- OK")
rep += sec

# ---- B. dependencies
KNOWN = {
 "symphonia": 'symphonia = { version = "0.5", default-features = false, features = ["mp3","aac","flac","vorbis","ogg","wav","isomp4","mkv","pcm"] }',
 "cpal": 'cpal = "0.15"',
 "image": 'image = { version = "0.24", default-features = false, features = ["png","jpeg","gif"] }',
 "rustybuzz": 'rustybuzz = "0.13"',
 "fontdb": 'fontdb = { version = "0.18", default-features = false, features = ["fs","memmap"] }',
 "mp4": 'mp4 = "0.14"',
 "rusqlite": 'rusqlite = { version = "0.30", features = ["bundled"] }',
 "tiny_skia": 'tiny-skia = { version = "0.11", default-features = false, features = ["std"] }',
 "html5ever": 'html5ever = "0.26"', "url": 'url = "2.4"', "encoding_rs": 'encoding_rs = "0.8"',
 "boa_engine": 'boa_engine = { version = "0.17", default-features = false }',
 "taffy": 'taffy = { version = "0.4", default-features = false, features = ["std","taffy_tree","flexbox","block_layout"] }',
 "reqwest": 'reqwest = { version = "0.11", default-features = false, features = ["blocking","rustls-tls","cookies"] }',
}
cargo = root / "Cargo.toml"
ctext = read(cargo)
have = {m.group(1).replace("-", "_") for m in re.finditer(r"^\s*([A-Za-z0-9_\-]+)\s*=", ctext, flags=re.M)}
used = {c for c in KNOWN if re.search(rf"\b{c}::", allcode) or re.search(rf"\buse\s+{c}\b", allcode)}
miss = sorted(used - have)
rep += ["", "## B. Missing dependencies in Cargo.toml"] + ([f"- `{c}` -> `{KNOWN[c]}`" for c in miss] or ["- OK"])
if "cookie_store" in allcode and "cookies" not in ctext:
    rep.append("- reqwest needs feature `cookies` (cookie_store is used)")
if FIX and miss:
    backup(cargo)
    t = ctext.replace("[profile.release]", "\n".join(KNOWN[c] for c in miss) + "\n\n[profile.release]", 1)
    cargo.write_text(t, encoding="utf-8"); applied.append("Cargo.toml: + " + ", ".join(miss))
if FIX and "[lib]" not in read(cargo) and (src / "ffi.rs").exists():
    backup(cargo)
    t = read(cargo).replace("[dependencies]", '[lib]\ncrate-type = ["rlib", "cdylib"]\n\n[dependencies]', 1)
    cargo.write_text(t, encoding="utf-8"); applied.append("Cargo.toml: + [lib] crate-type rlib,cdylib")

# ---- C. cross-module references
sec = ["", "## C. References to items that do not exist (`dro_x::name`)"]
seen = set()
for p, c in code.items():
    for m in re.finditer(r"\b(dro_[a-z_]+)::([A-Za-z_][A-Za-z0-9_]*)", c):
        mod, name = m.groups()
        tgt = src / f"{mod}.rs"
        if not tgt.exists(): key = (mod, "*")
        else:
            if re.search(rf"\b(fn|struct|enum|const|static|type|mod|trait)\s+{name}\b", defs[tgt]) or re.search(rf"\buse\s.*\b{name}\b", defs[tgt]): continue
            key = (mod, name)
        if (key, p.name) in seen: continue
        seen.add((key, p.name))
        sec.append(f"- `{p.relative_to(src)}` uses `{mod}::{name}` -> " + ("module file missing" if key[1] == "*" else f"`{name}` not defined in `{mod}.rs`"))
if len(sec) == 2: sec.append("- OK")
rep += sec

# ---- D. engine fields/methods used by child modules
eng = src / "sparrow_engine.rs"
sec = ["", "## D. Engine fields/methods used by child modules but not defined (possible)"]
if eng.exists():
    ec = code[eng]
    m = re.search(r"pub struct SparrowEngine\s*\{(.*?)\n\}", ec, flags=re.S)
    fields = set(re.findall(r"^\s*(?:pub\s+)?([a-z_][a-z0-9_]*)\s*:", m.group(1), flags=re.M)) if m else set()
    fns = set(re.findall(r"\bfn\s+([a-z_][a-z0-9_]*)", "\n".join(c for p, c in code.items() if p == eng or p.parent == src / "sparrow_engine")))
    for p in sorted((src / "sparrow_engine").glob("*.rs")):
        miss = sorted({f for f in re.findall(r"\b(?:self|e)\.([a-z_][a-z0-9_]*)\b(?!\s*[(:<])", code[p])} - fields - fns)
        if miss: sec.append(f"- `{p.name}`: {', '.join(miss)}")
if len(sec) == 2: sec.append("- OK")
rep += sec

# ---- E. disk access
sec = ["", "## E. Disk access (goal: RAM-only except bookmarks/settings)"]
for p, c in code.items():
    for pat, lab in ((r"Connection::open\(", "SQLite file"), (r"\bfs::(write|File|create)|std::fs::", "fs access"), (r"File::create", "File::create")):
        if re.search(pat, c): sec.append(f"- `{p.relative_to(src)}`: {lab}")
if len(sec) == 2: sec.append("- OK")
rep += sec

out = ["# Integration report", f"crate: `{root}`", ""] + (["## Applied (--fix-safe)"] + [f"- {a}" for a in applied] + [""] if applied else []) + rep
(root / "INTEGRATION_REPORT.md").write_text("\n".join(out) + "\n", encoding="utf-8")
print("\n".join(out))
