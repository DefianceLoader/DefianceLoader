"""Derive a build's object-layout offset changes from the one the patch was
written for, by aligning the functions the two builds share.

A game update moves functions but, where a class gained members, it also
changes the small struct offsets and vtable slots instructions use. Those
offsets are what a per-build layout table has to carry, because the patch code
reads the same fields. This tool pairs each reference `.pdata` function with
its counterpart in another build (the pairing `tools/builddiff.py` uses, plus
functions whose size changed because a displacement outgrew one byte), aligns
their instructions, and collects every non-stack displacement that differs
together with the instruction shape it sits in. A shape that maps one
old offset to one new offset everywhere is unambiguous; a shape seen with two
is ambiguous and reported with the addresses of the occurrences that moved, so
it is resolved by hand.

The whole analysis is one pass over both modules and is cached, keyed by the
two modules' hashes and this tool's version, because the same questions are
asked repeatedly while a build is ported.

    python tools/offsetmap.py logic bin/gog/logic-updated.dll
    python tools/offsetmap.py game  bin/gog/game-updated.dll --occurrences 0x160 "call|rax|0x148"
    python tools/offsetmap.py logic bin/gog/logic-updated.dll --json out/offsetmap-logic.json
"""
import bisect, collections, hashlib, json, pathlib, re, sys
import capstone
sys.path.insert(0, "tools")
import sigs
from builddiff import REFERENCE, functions, normalized, shape

# Bump when the analysis changes shape, so a stale cache is not reused.
VERSION = 3
CACHE = pathlib.Path("out")
# A function paired by its skeleton alone must be at least this long, so two
# small accessors that differ only in the field they read are not confused.
MIN_SKELETON = 12
NUMBER = re.compile(r"-?0x[0-9a-f]+|\b\d+\b")


def skeleton(ins):
    """An instruction with its numbers blanked: the mnemonic and registers,
    whatever immediates and displacements it carries or how they encode."""
    return ins.mnemonic, NUMBER.sub("N", ins.op_str)


def align(ref, other, rs, re_, ts, te):
    """(old instruction, new instruction) pairs, walking both functions in
    step while the mnemonics agree and either the sizes agree or the two differ
    only in their numbers: a field that moved past 0x7f makes its displacement
    four bytes instead of one, and that move is exactly what is wanted. Stops
    when they fall out of step."""
    out = []
    for x, y in zip(ref.md.disasm(ref.image[rs:re_], rs), other.md.disasm(other.image[ts:te], ts)):
        if x.mnemonic != y.mnemonic or (x.size != y.size and skeleton(x) != skeleton(y)):
            break
        out.append((x, y))
    return out


def non_rip(ins):
    """(base, index, disp) for each memory operand that names a class field:
    not rip-relative and not a stack frame, which the payload never rewrites."""
    stack = (capstone.x86.X86_REG_RIP, capstone.x86.X86_REG_RSP, capstone.x86.X86_REG_RBP)
    return [(op.mem.base, op.mem.index, op.mem.disp) for op in ins.operands
            if op.type == capstone.x86.X86_OP_MEM and op.mem.base not in stack]


def pairs_for(which, other_path):
    """[(reference function start, other function start, [(old, new), ...]), ...]
    for every function the two builds share."""
    ref, other = sigs.Module(REFERENCE[which][0]), sigs.Module(other_path)
    rf, of = functions(ref), functions(other)
    where = collections.defaultdict(list)
    for s, e in of:
        where[normalized(other, s, e)].append(s)
    shifts, unmatched = collections.Counter(), []
    # normalized-hash pairing: identical or data-reference-only functions
    direct = {}
    for s, e in rf:
        found = where.get(normalized(ref, s, e))
        if found:
            t = min(found, key=lambda x: abs(x - s))
            direct[s] = t
            shifts[t - s] += 1
        else:
            unmatched.append((s, e))
    starts, ends = [s for s, _ in of], dict(of)
    common = [d for d, _ in shifts.most_common(12)]
    for s, e in unmatched:
        # a changed function: same size at a common shift, same instruction shape
        for d in common:
            i = bisect.bisect_left(starts, s + d)
            for j in (i - 1, i):
                if 0 <= j < len(starts) and abs(starts[j] - (s + d)) < 0x40 \
                        and ends[starts[j]] - starts[j] == e - s \
                        and shape(other, starts[j], ends[starts[j]]) == shape(ref, s, e):
                    direct[s] = starts[j]
                    break
            else:
                continue
            break
    # A changed function whose size changed too, as it does when a field
    # crosses the one-byte displacement range: the same instructions with only
    # their numbers changed, and the only such pair on either side.
    taken = set(direct.values())
    def skeletons(module, spans):
        index = collections.defaultdict(list)
        for s, e in spans:
            ins = list(module.md.disasm(module.image[s:e], s))
            if len(ins) >= MIN_SKELETON:
                index[tuple(skeleton(i) for i in ins)].append(s)
        return index
    ref_left = skeletons(ref, [(s, e) for s, e in unmatched if s not in direct])
    other_left = skeletons(other, [(s, e) for s, e in of if s not in taken])
    for key, found in ref_left.items():
        if len(found) == 1 and len(other_left.get(key, ())) == 1:
            direct[found[0]] = other_left[key][0]
    out = []
    for s, e in rf:
        t = direct.get(s)
        if t is None:
            continue
        te = ends[t]
        out.append((s, t, align(ref, other, s, e, t, te)))
    return out


def _sha(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def analyze(which, other_path, refresh=False):
    """The cached analysis of `other_path` against the reference build, or a
    fresh one. Returns {version, ref_sha, tgt_sha, unambiguous:{key:new},
    ambiguous:{key:{new:count}}, occurrences:{key:[[new, ref_fn, ref_ins,
    tgt_fn, tgt_ins], ...]}}.

    The key ignores the index register, so an indexed operand and a simple one
    that share a displacement collapse and read as ambiguous. That is
    deliberate: `build.apply_layout` refuses to rewrite an indexed operand a key
    claims, because the two may be different classes' fields. `occurrences` is
    only the changed ones, with the function and instruction of each, so an
    ambiguous key can be resolved by inspecting the calls that moved."""
    ref_sha, tgt_sha = _sha(REFERENCE[which][0]), _sha(other_path)
    cache = CACHE / f"offsetmap-analysis-{which}-{ref_sha[:8]}-{tgt_sha[:8]}-v{VERSION}.json"
    if cache.exists() and not refresh:
        return json.loads(cache.read_text(encoding="utf-8"))
    table = collections.defaultdict(collections.Counter)
    occurrences = collections.defaultdict(list)
    for s, t, pairs in pairs_for(which, other_path):
        for old, new in pairs:
            dx, dy = non_rip(old), non_rip(new)
            if len(dx) != len(dy):
                continue
            for a, b in zip(dx, dy):
                # Sameness of base and index is what ties the two operands
                # together; an unchanged occurrence still counts, so a class
                # that kept its offset keeps the key ambiguous.
                if (a[0], a[1]) != (b[0], b[1]):
                    continue
                table[(old.mnemonic, a[0], a[2])][b[2]] += 1
                if b[2] != a[2]:
                    occurrences[(old.mnemonic, a[0], a[2])].append(
                        [b[2], s, old.address, t, new.address])
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    encode = lambda key: f"{key[0]}|{md.reg_name(key[1]) if key[1] else ''}|{key[2]:#x}"
    data = {
        "version": VERSION,
        "ref_sha": ref_sha,
        "tgt_sha": tgt_sha,
        "unambiguous": {encode(k): next(iter(c)) for k, c in table.items()
                        if len(c) == 1 and next(iter(c)) != k[2]},
        "ambiguous": {encode(k): dict(c) for k, c in table.items() if len(c) > 1},
        "occurrences": {encode(k): v for k, v in occurrences.items()},
    }
    CACHE.mkdir(exist_ok=True)
    cache.write_text(json.dumps(data, indent=2, sort_keys=True), encoding="utf-8")
    return data


def collect(which, other_path, refresh=False):
    """{key: Counter(new disp)} over every aligned pair, for callers that want
    the raw counts. Named keys are as `analyze`."""
    data = analyze(which, other_path, refresh)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    table = collections.defaultdict(collections.Counter)
    for name, value in data["unambiguous"].items():
        table[name][value] += 1
    for name, counts in data["ambiguous"].items():
        for value, count in counts.items():
            table[name][int(value)] += count
    return table


def main():
    which, other = sys.argv[1], sys.argv[2]
    data = analyze(which, other, refresh="--refresh" in sys.argv)
    if "--json" in sys.argv:
        path = sys.argv[sys.argv.index("--json") + 1]
        pathlib.Path(path).write_text(json.dumps({k: data[k] for k in ("unambiguous", "ambiguous")},
                                                 indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(f"wrote {path}")
        return
    if "--occurrences" in sys.argv:
        # a full key (`call|rax|0x148`), or a bare displacement for every key
        # that carries it
        wanted = sys.argv[sys.argv.index("--occurrences") + 1:]
        for key in wanted:
            names = [key] if "|" in key else sorted(
                name for name in data["occurrences"] if name.rsplit("|", 1)[1] == f"{int(key, 16):#x}")
            names = [name for name in names if name in data["occurrences"]]
            if not names:
                print(f"{key}: no changed occurrence")
            for name in names:
                print(f"{name}:")
                for new, s, a, t, b in data["occurrences"][name]:
                    print(f"  -> {new:#x}   ref fn_{s:x}+{a-s:#x}  target fn_{t:x}+{b-t:#x}")
        return
    print(f"{which}: {len(data['unambiguous'])} unambiguous offset changes, "
          f"{len(data['ambiguous'])} ambiguous")
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    for name, value in sorted(data["unambiguous"].items(), key=lambda kv: kv[0]):
        print(f"  {name} -> {value:#x}")
    for name, counts in sorted(data["ambiguous"].items(), key=lambda kv: kv[0]):
        print(f"  AMBIG {name} -> {{{', '.join(f'{int(k):#x}: {v}' for k, v in sorted(counts.items()))}}}")


if __name__ == "__main__":
    main()
