"""Write SHA256SUMS for release archives and the executables inside them.

    python tools/checksums.py out/defiance-loader-v1.2.0.zip [...] --out out/SHA256SUMS.txt

Each archive is listed by name, then each DLL and EXE it contains as
`archive!path`, so a scanned or extracted binary can be matched to the release.
The format is `sha256sum`'s: `<hash>  <name>`.
"""
import argparse
import hashlib
import pathlib
import sys
import zipfile

EXECUTABLE = ('.dll', '.exe')


def lines(archives):
    for archive in archives:
        archive = pathlib.Path(archive)
        yield f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}'
        with zipfile.ZipFile(archive) as contents:
            for name in sorted(contents.namelist()):
                if name.lower().endswith(EXECUTABLE):
                    digest = hashlib.sha256(contents.read(name)).hexdigest()
                    yield f'{digest}  {archive.name}!{name}'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('archives', nargs='+', type=pathlib.Path)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    args = parser.parse_args(argv)
    text = '\n'.join(lines(args.archives)) + '\n'
    args.out.write_text(text, encoding='utf-8', newline='\n')
    sys.stdout.write(text)
    return 0


if __name__ == '__main__':
    sys.exit(main())
