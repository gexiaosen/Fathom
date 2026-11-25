#!/usr/bin/env python3
"""Convert the Web single-file floor plan (index.html) into a Document fixture.

Usage:
    python3 scripts/import_web_fixture.py

Reads ../legacy/web/index.html and writes assets/samples/demo-apartment.json.

The Web prototype stores each wall as the *diagonal* of a 240mm-thick
rectangle, e.g. [1580,-240,2120,0]. The Rust model stores centrelines, so this
script converts each diagonal into (start, end, thickness) by keeping the long
axis and centring the short one. Windows and doors are intentionally not
migrated yet: the Web data has no wall ids for them, and the Rust editor will
create them against walls it draws itself (Sprint 3).
"""

from __future__ import annotations

import json
import pathlib
import re

APP = pathlib.Path(__file__).resolve().parents[1]
REPO = APP.parent
OUT = APP / "assets" / "samples" / "demo-apartment.json"

KIND = {"b": "Bearing", "e": "Exterior", "n": "Partition", "low": "Low"}
DEFAULT_THICKNESS = 240
WALL_HEIGHT = 2800
LOW_WALL_HEIGHT = 1000


def block(start: str, end: str) -> str:
    text = (REPO / "legacy" / "web" / "index.html").read_text(encoding="utf-8")
    i = text.index(start)
    j = text.index(end, i)
    return text[i:j]


def parse_walls() -> list[dict]:
    src = block("const WALLS = [", "];")
    rows = re.findall(r"\[(-?\d+),(-?\d+),(-?\d+),(-?\d+),'(b|e|n|low)'\]", src)
    walls = []
    for idx, (x0, y0, x1, y1, kind) in enumerate(rows, start=1):
        x0, y0, x1, y1 = int(x0), int(y0), int(x1), int(y1)
        dx, dy = abs(x1 - x0), abs(y1 - y0)
        thickness = max(120, min(dx, dy)) if min(dx, dy) >= 100 else DEFAULT_THICKNESS
        if dx >= dy:
            cy = (y0 + y1) // 2
            start, end = (x0, cy), (x1, cy)
        else:
            cx = (x0 + x1) // 2
            start, end = (cx, y0), (cx, y1)
        walls.append(
            {
                "id": idx,
                "start": {"x": start[0], "y": start[1]},
                "end": {"x": end[0], "y": end[1]},
                "thickness_mm": thickness,
                "kind": KIND[kind],
                "height_mm": LOW_WALL_HEIGHT if kind == "low" else WALL_HEIGHT,
            }
        )
    return walls


def rect_points(x0: int, y0: int, x1: int, y1: int) -> list[dict]:
    return [
        {"x": x0, "y": y0},
        {"x": x1, "y": y0},
        {"x": x1, "y": y1},
        {"x": x0, "y": y1},
    ]


def parse_materials() -> tuple[list[dict], dict[str, int]]:
    src = block("const MATS = {", "};")
    mats = []
    ids = {}
    for key, name, price in re.findall(r"(\w+):\s*\{name:'([^']+)',\s*price:(\d+)", src):
        mat_id = 2000 + len(mats) + 1
        ids[key] = mat_id
        mats.append({"id": mat_id, "name": name, "price_per_m2": float(price), "kind": key})
    return mats, ids


def parse_rooms(mat_ids: dict[str, int]) -> list[dict]:
    src = block("const ROOMS = [", "];")
    rooms = []
    for chunk in src.split("{id:'")[1:]:
        rid = chunk.split("'", 1)[0]
        name = re.search(r"name:'([^']+)'", chunk).group(1)
        mat = re.search(r"mat:'([^']+)'", chunk)
        counted = "counted:false" not in chunk

        r_match = re.search(r"poly:R\((-?\d+),(-?\d+),(-?\d+),(-?\d+)\)", chunk)
        if r_match:
            poly = rect_points(*map(int, r_match.groups()))
        else:
            poly_src = re.search(r"poly:\[(.*?)\]\s*,\s*mat", chunk, re.S).group(1)
            pts = re.findall(r"\[(-?\d+),(-?\d+)\]", poly_src)
            poly = [{"x": int(x), "y": int(y)} for x, y in pts]

        rooms.append(
            {
                "id": 1000 + len(rooms) + 1,
                "polygon": poly,
                "name": name,
                "material_id": mat_ids.get(mat.group(1)) if mat else None,
                "counted": counted,
                "_web_id": rid,
            }
        )
    return rooms


def main() -> None:
    walls = parse_walls()
    mats, mat_ids = parse_materials()
    rooms = parse_rooms(mat_ids)

    doc = {
        "schema_version": 1,
        "meta": {
            "name": "示例户型（Web 版导入）",
            "client": "",
            "address": "",
            "note": "由 scripts/import_web_fixture.py 从 legacy/web/index.html 生成",
            "created_at": "2026-11-20",
            "updated_at": "2026-11-20",
        },
        "site": {"boundary": [], "floor": 1, "unit": "Millimetre"},
        "walls": walls,
        "openings": [],
        "rooms": rooms,
        "furniture": [],
        "materials": mats,
        "measures": [],
        "settings": {"grid_mm": 10, "snap": True, "show_dimensions": True},
    }

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(doc, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {OUT.relative_to(APP)}")
    print(f"  walls={len(walls)} rooms={len(rooms)} materials={len(mats)}")
    total = sum(
        abs(
            sum(
                p["x"] * poly[(i + 1) % len(poly)]["y"] - poly[(i + 1) % len(poly)]["x"] * p["y"]
                for i, p in enumerate(poly)
            )
        )
        / 2
        for poly in ([r["polygon"] for r in rooms if r["counted"]])
        for p in [poly]
    )
    print(f"  counted room area: {total/1e6:.1f} m2")


if __name__ == "__main__":
    main()
