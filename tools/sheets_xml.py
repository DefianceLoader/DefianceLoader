"""Rewrite the game's Excel 2003 SpreadsheetML tables as plain XML.

The unpacked paks keep their data tables as SpreadsheetML: every value sits
under styles, column widths and workbook metadata. This keeps only the sheets,
rows and values. Each worksheet's first non-empty row is its header; every
later row becomes <row> with one child per non-empty cell, named after its
header. A header that is not a valid, unique XML name becomes
<field name="...">, and a cell under an empty header becomes <col n="N">.
Cell comments and formulas are kept as note= and formula= attributes. Blank
rows and data types are dropped. XML that is not SpreadsheetML is copied.

    python tools/sheets_xml.py <unpacked dir> <output dir>

tools/sheets_tsv.py writes the same tables as TSV.
"""

import re
import shutil
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

SS = "{urn:schemas-microsoft-com:office:spreadsheet}"
NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_.\-]*$")
# Threaded-comment boilerplate Excel prepends; the real note follows the marker.
COMMENT_MARKERS = ("Комментарий:", "Comment:")


def comment_text(cell):
    c = cell.find(SS + "Comment")
    if c is None:
        return None
    text = "".join(c.itertext())
    for m in COMMENT_MARKERS:
        if m in text:
            text = text.split(m, 1)[1]
            break
    text = text.strip()
    return text or None


def read_rows(table):
    rows = []
    r = 0
    for row in table.findall(SS + "Row"):
        idx = row.get(SS + "Index")
        r = int(idx) if idx else r + 1
        cells = {}
        c = 0
        for cell in row.findall(SS + "Cell"):
            idx = cell.get(SS + "Index")
            c = int(idx) if idx else c + 1
            data = cell.find(SS + "Data")
            value = "".join(data.itertext()) if data is not None else None
            note = comment_text(cell)
            formula = cell.get(SS + "Formula")
            if value is not None or note or formula:
                cells[c] = (value or "", note, formula)
            c += int(cell.get(SS + "MergeAcross", 0))
        if cells:
            rows.append((r, cells))
    return rows


def add_cell(parent, tag, attrs, cell):
    value, note, formula = cell
    el = ET.SubElement(parent, tag, attrs)
    el.text = value
    if note:
        el.set("note", note)
    if formula:
        el.set("formula", formula)


def convert(src, dst):
    root = ET.parse(src).getroot()
    if root.tag != SS + "Workbook":
        return False
    out = ET.Element("workbook")
    for ws in root.findall(SS + "Worksheet"):
        sheet = ET.SubElement(out, "sheet", {"name": ws.get(SS + "Name", "")})
        table = ws.find(SS + "Table")
        rows = read_rows(table) if table is not None else []
        if not rows:
            continue
        _, header = rows[0]
        names = {}
        seen = {}
        for col, (v, _, _) in header.items():
            seen[v] = seen.get(v, 0) + 1
        for col, (v, _, _) in header.items():
            v = v.strip()
            if not v:
                continue
            names[col] = (v, v.lower() not in ("row", "col", "field")
                          and NAME_RE.match(v) is not None and seen[header[col][0]] == 1
                          and not v.lower().startswith("xml"))
        hdr = ET.SubElement(sheet, "header")
        for col in sorted(header):
            add_cell(hdr, "col", {"n": str(col)}, header[col])
        for _, cells in rows[1:]:
            row_el = ET.SubElement(sheet, "row")
            for col in sorted(cells):
                if col in names:
                    name, ok = names[col]
                    if ok:
                        add_cell(row_el, name, {}, cells[col])
                    else:
                        add_cell(row_el, "field", {"name": name}, cells[col])
                else:
                    add_cell(row_el, "col", {"n": str(col)}, cells[col])
    ET.indent(out, "  ")
    dst.parent.mkdir(parents=True, exist_ok=True)
    ET.ElementTree(out).write(dst, encoding="utf-8", xml_declaration=True)
    return True


def main():
    src_root, dst_root = Path(sys.argv[1]), Path(sys.argv[2])
    converted = copied = 0
    for src in sorted(src_root.rglob("*.xml")):
        dst = dst_root / src.relative_to(src_root)
        if convert(src, dst):
            converted += 1
        else:
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
            copied += 1
    print(f"converted {converted}, copied unchanged {copied}")


if __name__ == "__main__":
    main()
