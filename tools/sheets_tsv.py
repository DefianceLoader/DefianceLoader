"""Rewrite the game's Excel 2003 SpreadsheetML tables as TSV, one file per
worksheet.

Columns keep their spreadsheet positions (a skipped cell is an empty field);
blank rows and empty sheets are dropped. A workbook with one non-empty sheet
becomes name.tsv, one with several becomes name.<sheet>.tsv per sheet. Line
breaks inside a cell are written as ¶; everything else is verbatim. Comments
and formulas are not kept: tools/sheets_xml.py keeps them.

    python tools/sheets_tsv.py <unpacked dir> <output dir>
"""

import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

from sheets_xml import SS, read_rows

BAD_FILENAME = re.compile(r'[<>:"/\\|?*\x00-\x1f]')


def escape(v):
    # Game paths use literal backslashes (voices\target.wav), so \n-style
    # escapes would be ambiguous; the source contains neither ¶ nor tabs.
    assert "¶" not in v and "\t" not in v, v
    return v.replace("\r\n", "¶").replace("\r", "¶").replace("\n", "¶")


def sheet_tsv(rows):
    lines = []
    for _, cells in rows:
        width = max(cells)
        lines.append("\t".join(escape(cells[c][0]) if c in cells else ""
                               for c in range(1, width + 1)))
    return "\n".join(lines) + "\n"


def main():
    src_root, dst_root = Path(sys.argv[1]), Path(sys.argv[2])
    files = sheets = 0
    for src in sorted(src_root.rglob("*.xml")):
        root = ET.parse(src).getroot()
        if root.tag != SS + "Workbook":
            continue
        out = []
        for ws in root.findall(SS + "Worksheet"):
            table = ws.find(SS + "Table")
            rows = read_rows(table) if table is not None else []
            if rows:
                out.append((ws.get(SS + "Name", ""), sheet_tsv(rows)))
        rel = src.relative_to(src_root)
        dst_dir = (dst_root / rel).parent
        dst_dir.mkdir(parents=True, exist_ok=True)
        used = set()
        for name, text in out:
            if len(out) == 1:
                fname = rel.stem + ".tsv"
            else:
                safe = BAD_FILENAME.sub("_", name).strip() or "sheet"
                fname = f"{rel.stem}.{safe}.tsv"
                n = 2
                while fname.lower() in used:
                    fname = f"{rel.stem}.{safe}_{n}.tsv"
                    n += 1
            used.add(fname.lower())
            (dst_dir / fname).write_text(text, encoding="utf-8", newline="\n")
            sheets += 1
        files += 1
    print(f"{files} workbooks -> {sheets} TSV files")


if __name__ == "__main__":
    main()
