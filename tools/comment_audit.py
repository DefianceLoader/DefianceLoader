"""Find stale comments in the tracked sources.

    python tools/comment_audit.py --check   fail on comments naming missing paths
    python tools/comment_audit.py           also report comments to review

A comment can only be proven stale where it names something checkable. The
check fails when a comment names a repository path (`tools/x.py`, `crates/...`)
that does not exist; rustdoc covers Rust item references (CI builds the docs
with every rustdoc warning, broken links included, denied). Write references in
those forms so they stay checked.

The report lists what needs a human look, and does not fail:

- history phrasing ("no longer", "used to", "began as", "previously", dates,
  commit hashes): what code used to do belongs in the commit message or the
  notes, not beside the code, where it goes stale quietly;
- comment blocks whose following code was changed (per `git blame`, ignoring
  the revisions in `.git-blame-ignore-revs`) after the comment last was: the
  comment may describe the old code.
"""
import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCES = ('*.rs', '*.py', '*.asm', '*.toml', '*.yml')
COMMENT = {'.rs': re.compile(r'^\s*//[/!]?(.*)$|\S.*?\s//[/!]?(.*)$'),
           '.py': re.compile(r'^\s*#(.*)$|\S.*?\s#\s(.*)$'),
           '.asm': re.compile(r'^\s*;(.*)$|\S.*?;(.*)$'),
           '.toml': re.compile(r'^\s*#(.*)$'),
           '.yml': re.compile(r'^\s*#(.*)$')}
# A repository path a comment names.
PATH = re.compile(r'(?<![\w./-])((?:crates|plugins|tools|patch|injector|examples|docs|notes)'
                  r'/[\w./-]*\w\.(?:md|py|rs|asm|toml|json|ini|yml))\b')
# "used to" after "is/are/be/been" means "used for", and a signature that "no
# longer matches" describes a build; neither is history.
HISTORY = re.compile(r'(?i)\b(no longer(?! match)|(?<!is )(?<!are )(?<!be )(?<!been )used to|began as|'
                     r'previously|formerly|was changed|until (?:now|recently)|'
                     r'20\d\d-\d\d-\d\d|commit [0-9a-f]{7,})\b')
# Paths that name things outside the checkout on purpose (examples, fixtures).
ALLOWED_MISSING = set()


def tracked():
    out = subprocess.run(['git', 'ls-files', *SOURCES], cwd=ROOT, capture_output=True,
                         text=True, check=True).stdout
    return [f for f in out.split() if (ROOT / f).is_file()]


def comments(path):
    """(line number, comment text) for every comment in a file."""
    rx = COMMENT[Path(path).suffix]
    for number, line in enumerate((ROOT / path).read_text(encoding='utf-8', errors='replace')
                                  .splitlines(), 1):
        match = rx.match(line)
        if match:
            yield number, next(group for group in match.groups() if group is not None)


def missing_paths(files):
    found = []
    for path in files:
        for number, text in comments(path):
            for named in PATH.findall(text):
                if named not in ALLOWED_MISSING and not (ROOT / named).exists():
                    found.append(f'{path}:{number}: names {named}, which does not exist')
    return found


def history_phrasing(files):
    found = []
    for path in files:
        for number, text in comments(path):
            match = HISTORY.search(text)
            if match:
                found.append(f'{path}:{number}: "{match.group(0)}": {text.strip()[:100]}')
    return found


def blame_times(path):
    """The author time of each line, ignoring formatting-only revisions."""
    args = ['git', 'blame', '--line-porcelain']
    if (ROOT / '.git-blame-ignore-revs').is_file():
        args += ['--ignore-revs-file', '.git-blame-ignore-revs']
    out = subprocess.run([*args, '--', path], cwd=ROOT, capture_output=True, text=True,
                         encoding='utf-8', errors='replace')
    if out.returncode:
        return []
    return [int(line.split()[1]) for line in out.stdout.splitlines()
            if line.startswith('author-time ')]


def drifted(files, lines_after=12, days=3):
    """Comment blocks whose next code lines are more than `days` newer."""
    found = []
    for path in files:
        if Path(path).suffix not in ('.rs', '.py', '.asm'):
            continue
        times = blame_times(path)
        text = (ROOT / path).read_text(encoding='utf-8', errors='replace').splitlines()
        commented = {n for n, _ in comments(path)}
        n = 1
        while n <= len(text):
            if n not in commented or not re.match(r'^\s*(//|#|;)', text[n - 1]):
                n += 1
                continue
            start = n
            while n <= len(text) and n in commented and re.match(r'^\s*(//|#|;)', text[n - 1]):
                n += 1
            code = [i for i in range(n, min(n + lines_after, len(text) + 1))
                    if text[i - 1].strip() and i not in commented]
            if code and len(times) >= max(code) and n - start >= 2:
                comment_time = max(times[start - 1:n - 1])
                code_time = max(times[i - 1] for i in code)
                if code_time - comment_time > days * 86400:
                    found.append(f'{path}:{start}: code below changed '
                                 f'{(code_time - comment_time) // 86400} days after this comment')
    return found


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--check', action='store_true',
                        help='only the failing check (missing paths); for tests and CI')
    parser.add_argument('--days', type=int, default=3,
                        help='report code this much newer than its comment (default 3)')
    args = parser.parse_args(argv)
    files = tracked()
    problems = missing_paths(files)
    for line in problems:
        print(line)
    if args.check:
        print(f'{len(problems)} comment(s) name missing paths' if problems
              else 'comment paths: all exist')
        return 1 if problems else 0
    for title, found in (('history phrasing', history_phrasing(files)),
                         (f'code newer than its comment by over {args.days} days',
                          drifted(files, days=args.days))):
        print(f'\n{title}: {len(found)}')
        for line in found:
            print('  ' + line)
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
