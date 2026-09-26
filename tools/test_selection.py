"""Execute the actual selection edits and uninstalled member-filter prototype.

Uses fabricated native objects, never the running game. These checks establish
facet and filtering behavior, not client gestures, manager lifecycle, or AI.
Run on Windows x64 with the same environment as tools/test_chooser.py.
"""
import ctypes as C
import hashlib
import pathlib
import struct
import unittest

import build as b
import icon


class Native:
    def __init__(self):
        self.buffers = []
        self.pages = []
        self.kernel = C.WinDLL("kernel32", use_last_error=True)
        self.kernel.VirtualAlloc.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong, C.c_ulong]
        self.kernel.VirtualAlloc.restype = C.c_void_p
        self.kernel.VirtualFree.argtypes = [C.c_void_p, C.c_size_t, C.c_ulong]
        self.kernel.VirtualFree.restype = C.c_int

    def data(self, size, fields=()):
        buf = C.create_string_buffer(size)
        self.buffers.append(buf)
        addr = C.addressof(buf)
        for offset, value in fields:
            C.c_uint64.from_address(addr + offset).value = value
        return addr

    def code(self, blob):
        addr = self.kernel.VirtualAlloc(None, len(blob), 0x3000, 0x40)
        if not addr:
            raise C.WinError(C.get_last_error())
        self.pages.append(addr)
        C.memmove(addr, blob, len(blob))
        return addr

    def close(self):
        for addr in self.pages:
            if not self.kernel.VirtualFree(addr, 0, 0x8000):
                raise C.WinError(C.get_last_error())


class SelectionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        source = pathlib.Path(b.SRC).read_bytes()
        if hashlib.sha256(source).hexdigest() != b.EXPECT_SOURCE_SHA:
            raise RuntimeError("unsupported source DLL")
        cls.native = n = Native()
        img = b.Image(b.SRC)
        # Copy the whole selection region: all internal jumps are relative.
        region = bytearray(img.read(0x4491a0, 0x60))
        for rva, before, after in b.SELECTION_EDITS:
            if not 0x4491a0 <= rva < 0x449200:
                continue
            off = rva - 0x4491a0
            if region[off:off + len(before)] != before or len(before) != len(after):
                raise RuntimeError("unexpected selection patch bytes")
            region[off:off + len(before)] = after
        entry = n.code(bytes(region))
        # setSelected is not edited in place: the jmp over it leads to
        # patch/soldier-mark.asm, which is position independent.
        mark, _ = b.assemble(
            pathlib.Path("patch/soldier-mark.asm").read_text().splitlines(), 0, 0)
        setter = n.code(mark)
        cls.set_selected = C.CFUNCTYPE(None, C.c_void_p, C.c_ubyte)(setter)
        cls.is_selected = C.CFUNCTYPE(C.c_ubyte, C.c_void_p)(entry + 0x20)
        # Real manager clear-all: only internal branches and virtual calls,
        # so this entire routine can execute unchanged on fabricated entities.
        cls.clear_all = C.CFUNCTYPE(None, C.c_void_p)(
            n.code(img.read(0x4194f0, 0x51)))
        # Fake squad setter/getter with observable forwarding count at +0x34.
        squad_set = n.code(bytes.fromhex("885130ff4134c3"))
        squad_get = n.code(bytes.fromhex("0fb64130c3"))
        cls.squad_vt = n.data(0x60, [(0x50, squad_set), (0x58, squad_get)])
        cls.soldier_vt = n.data(0x60, [(0x50, setter), (0x58, entry + 0x20)])
        get_container = n.code(bytes.fromhex("488b4108c3"))
        cls.entity_vt = n.data(0xb8, [(0xb0, get_container)])
        blob, _ = b.assemble(pathlib.Path("patch/selection-filter.asm").read_text().splitlines(), 0, 0)
        cls.filter = C.CFUNCTYPE(C.c_void_p, C.c_void_p, C.c_void_p,
                                C.c_ubyte, C.c_void_p)(n.code(blob))
        # Execute the real manager select, including its type dispatch. Redirect
        # only the external owner-query and memcpy calls to controlled stubs.
        # Internal branches and virtual calls remain the actual engine bytes.
        def manager_select(patched):
            start, end = 0x418cb0, 0x418db0
            raw = bytearray(img.read(start, end - start))
            if patched:
                rva, before, after = next(
                    e for e in b.SELECTION_EDITS if e[0] == b.SQUAD_SELECT_RVA)
                off = rva - start
                assert raw[off:off + len(before)] == before
                raw[off:off + len(before)] = after
            owner_stub = len(raw)
            raw += bytes.fromhex("31c0c3")  # no special owner on fabricated men
            memcpy_stub = len(raw)
            raw += bytes.fromhex("56574889c84889cf4889d64c89c1f3a45f5ec3")
            for ins in img.md.disasm(bytes(raw[:end-start]), start):
                if ins.mnemonic == "call" and ins.op_str.startswith("0x"):
                    target = int(ins.op_str, 16)
                    offset = {0x9aea0: owner_stub, 0x68312a: memcpy_stub}[target]
                    assert ins.size == 5
                    struct.pack_into("<i", raw, ins.address - start + 1,
                                     start + offset - (ins.address + ins.size))
            return C.CFUNCTYPE(None, C.c_void_p, C.c_void_p)(n.code(bytes(raw)))
        cls.select_stock = manager_select(False)
        cls.select_patched = manager_select(True)
        type_query = n.code(bytes.fromhex("8b411023c20f95c0c3"))
        C.c_uint64.from_address(cls.entity_vt + 0x98).value = type_query
        C.c_uint64.from_address(cls.entity_vt + 0x50).value = n.code(
            bytes.fromhex("488b4118c3"))  # blueprint

        def routine(start, end, patched, targets, nargs):
            raw = bytearray(img.read(start, end - start))
            if patched:
                for rva, before, after in b.SELECTION_EDITS:
                    if start <= rva < end:
                        off = rva - start
                        assert raw[off:off + len(before)] == before
                        raw[off:off + len(before)] = after
            for ins in list(img.md.disasm(bytes(raw), start)):
                if ins.mnemonic == "call" and ins.op_str.startswith("0x"):
                    target = targets[int(ins.op_str, 16)]
                    trampoline = len(raw)
                    raw += b"\x48\xb8" + struct.pack("<Q", target) + b"\xff\xe0"
                    struct.pack_into("<i", raw, ins.address - start + 1,
                                     start + trampoline - (ins.address + ins.size))
            return C.CFUNCTYPE(None, *([C.c_void_p] * nargs))(n.code(bytes(raw)))

        # Stand in only for categorization/allocation: copy the supplied vector
        # into accepted bucket zero. Execute the real bucket consumer loop.
        categorize = n.code(bytes.fromhex("488b02488901488b420848894108c3"))
        targets = {0x418390: categorize, 0x55410: n.code(b"\xc3"),
                   0x418cb0: C.cast(cls.select_patched, C.c_void_p).value}
        cls.add_stock = routine(0x419c60, 0x419cef, False, targets, 2)
        cls.add_patched = routine(0x419c60, 0x419cef, True, targets, 2)
        # Region/ownership eligibility is controlled per entity; all actual
        # type comparisons, enabled checks and member selection remain intact.
        targets[0x418000] = n.code(bytes.fromhex("0fb64120c3"))
        cls.type_stock = routine(0x4191f0, 0x419348, False, targets, 3)
        cls.type_patched = routine(0x4191f0, 0x419348, True, targets, 3)
        # Execute the actual world hook together with the actual same-type
        # manager and squad selector, rather than testing their arguments alone.
        game_code, labels = b.assemble(
            pathlib.Path("patch/icon-squad.asm").read_text().splitlines(), 0, icon.TRACE_OFFSET,
            symbols=b.GAME_SYMBOLS)
        resume, _ = b.assemble(
            ["add rsp, 0x28", "pop r14", "pop rdi", "ret"], 0, 0)
        game_code = bytearray(game_code)
        at = game_code.index(struct.pack("<Q", 0xaaaaaaaaaaaaaaad))
        struct.pack_into("<Q", game_code, at, n.code(bytes.fromhex("31c0c3")))
        at = game_code.index(struct.pack("<Q", 0xaaaaaaaaaaaaaaa9))
        struct.pack_into("<Q", game_code, at, n.code(resume))
        game_entry = n.code(bytes(game_code).ljust(0x1000, b"\x00"))
        shim, _ = b.assemble([
            "push rdi", "push r14", "sub rsp, 0x28", "mov r14, rcx",
            "mov rdi, rdx", f"mov r11, {game_entry + labels['world_double']}",
            "jmp r11"], 0, 0)
        cls.world_double = C.CFUNCTYPE(None, C.c_void_p, C.c_void_p)(n.code(shim))
        cls.squad_of = C.CFUNCTYPE(C.c_void_p, C.c_void_p)(game_entry + labels["squad_of"])
        # only building_select runs here; the marquee's cell is never read
        building_code, building_labels = b.assemble(
            pathlib.Path("patch/region-individual.asm").read_text().splitlines(), 0, 0, 0x1000)
        building_set = n.code(building_code) + building_labels["building_select"]
        cls.building_vt = n.data(0x60, [(0x50, building_set),
            (0x58, n.code(img.read(0x1d7590, 0x12)))])
        cls.building_set = C.CFUNCTYPE(None, C.c_void_p, C.c_ubyte)(building_set)
        cls.building_stock = C.CFUNCTYPE(None, C.c_void_p, C.c_ubyte)(n.code(img.read(0x1d7ff0, 4)))
        # Real native TeamFacet own-team predicate: relation zero only.
        cls.team_vt = n.data(0x88, [(0x80, n.code(img.read(0x5ffe0, 11)))])

    @classmethod
    def tearDownClass(cls):
        cls.native.close()

    def facet(self, squad=0, enabled=True, marked=False, vt=None):
        return self.native.data(0x40, [(0, self.soldier_vt if vt is None else vt),
                                     (0x18, int(enabled)), (0x28, squad),
                                     (0x30, int(marked))])

    def entity(self, facet):
        container = self.native.data(0x58, [(0x50, facet)])
        return self.native.data(0x28, [(0, self.entity_vt), (8, container),
                                       (0x18, 1), (0x20, 1)])

    def squad_entity(self, members, facet):
        n = self.native
        pointers = n.data(max(8, len(members) * 8),
                          [(i * 8, m) for i, m in enumerate(members)])
        vector = n.data(0x18, [(0, pointers), (8, pointers + len(members) * 8)])
        getter = n.code(bytes.fromhex("488b4108c3"))
        roster_vt = n.data(0x70, [(0x68, getter)])
        roster = n.data(0x10, [(0, roster_vt), (8, vector)])
        ai_vt = n.data(0x3c0, [(0x3b8, getter)])
        ai = n.data(0x10, [(0, ai_vt), (8, roster)])
        container = n.data(0x58, [(0x28, ai), (0x50, facet)])
        return n.data(0x28, [(0, self.entity_vt), (8, container), (0x10, 0x10),
                             (0x18, 2), (0x20, 1)])

    def vector(self, entities):
        begin = self.native.data(max(8, len(entities) * 8),
                                 [(i * 8, e) for i, e in enumerate(entities)])
        return self.native.data(0x18, [(0, begin), (8, begin + len(entities) * 8)])

    def building(self, occupants):
        n = self.native
        facet = self.facet(vt=self.building_vt)
        entity = self.entity(facet)
        C.c_uint64.from_address(entity + 0x10).value = 0x200
        holder = n.data(0x18, [(0x10, entity)])
        C.c_uint64.from_address(facet + 0x10).value = holder
        refs = [n.data(0x18, [(0x10, e)]) if e else 0 for e in occupants]
        array = n.data(max(8, len(refs)*8), [(i*8, r) for i,r in enumerate(refs)])
        active = n.data(0x1c0, [(0x1a8,array), (0x1b0,array+8*len(refs))])
        tangible = n.data(0x168, [(0x160,active)])
        facets = C.c_uint64.from_address(entity+8).value
        C.c_uint64.from_address(facets+0x10).value = tangible
        return entity, facet

    def test_building_click_preserves_building_despite_manager_pointer(self):
        building, facet = self.building([])
        unrelated = self.entity(self.facet())
        holder = self.native.data(0x18, [(0x10, unrelated)])
        manager = self.native.data(0x38, [(0x10, holder)])
        C.c_uint64.from_address(facet+0x28).value = manager
        self.assertEqual(self.squad_of(building), building)

    def test_building_selection_preserves_occupant_and_squad_selection(self):
        n = self.native
        squad_a, a = self.linked_squad(3)
        squad_b, b = self.linked_squad(2)
        # Two squads split between this building, another building, and outside.
        C.c_ubyte.from_address(squad_a + 0x30).value = 0
        C.c_ubyte.from_address(squad_b + 0x30).value = 0
        inside = [self.entity(a[0]), self.entity(b[1])]
        for entity in inside:
            team = n.data(0x180, [(0, self.team_vt), (0x178, 0)])
            facets = C.c_uint64.from_address(entity + 8).value
            C.c_uint64.from_address(facets + 0x20).value = team
        building, facet = self.building(inside)
        _, other_facet = self.building([self.entity(a[1])])
        watched = [squad_a, squad_b, *a, *b, other_facet]
        for outside_selected in (False, True):
            if outside_selected:
                self.set_selected(a[2], 1)
            before = [C.string_at(pointer, 0x38) for pointer in watched]
            self.select_patched(None, building)
            self.assertEqual(C.c_ubyte.from_address(facet + 0x30).value, 1)
            self.assertEqual([C.string_at(pointer, 0x38) for pointer in watched], before,
                             "building click must not promote occupant squads into selection")
            self.building_set(facet, 0)
            self.assertEqual(C.c_ubyte.from_address(facet + 0x30).value, 0)
            self.assertEqual([C.string_at(pointer, 0x38) for pointer in watched], before,
                             "building deselect must not alter independently selected soldiers")

    def test_building_set_matches_native_flag_only_contract(self):
        # Stock setter does not follow holder, occupant or manager pointers.
        # Fill everything else with invalid pointer bytes to catch extra reads.
        stock = self.native.data(0x40)
        patched = self.native.data(0x40)
        for value in (0, 1, 127, 255):
            C.memset(stock, 0xa5, 0x40)
            C.memset(patched, 0xa5, 0x40)
            self.building_stock(stock, value)
            self.building_set(patched, value)
            self.assertEqual(C.string_at(stock, 0x40), C.string_at(patched, 0x40))

    def test_building_empty_or_missing_state_still_sets_native_flag(self):
        building, facet = self.building([])
        for missing in (False, True):
            if missing:
                C.c_uint64.from_address(facet+0x10).value = 0
            for value in (1,0):
                self.building_set(facet, value)
                self.assertEqual(C.c_ubyte.from_address(facet+0x30).value, value)

    def test_shift_add_marks_all_members_and_readd_restores_marks(self):
        squad = self.facet(vt=self.squad_vt)
        facets = [self.facet(squad) for _ in range(3)]
        entity = self.squad_entity([self.entity(f) for f in facets], squad)
        vector = self.vector([entity])
        self.add_stock(None, vector)
        self.assertEqual(C.c_ubyte.from_address(squad + 0x30).value, 1)
        self.assertEqual([self.is_selected(f) for f in facets], [0, 0, 0])
        self.add_patched(None, vector)
        self.assertEqual([self.is_selected(f) for f in facets], [1, 1, 1])
        C.c_ubyte.from_address(squad + 0x30).value = 0
        self.assertEqual([self.is_selected(f) for f in facets], [0, 0, 0])
        self.add_patched(None, vector)
        self.assertEqual([self.is_selected(f) for f in facets], [1, 1, 1])

    def test_shift_add_individual_and_empty_bucket(self):
        squad = self.facet(vt=self.squad_vt)
        facets = [self.facet(squad) for _ in range(3)]
        self.add_patched(None, self.vector([self.entity(facets[1])]))
        self.add_patched(None, self.vector([]))
        self.assertEqual([self.is_selected(f) for f in facets], [0, 1, 0])

    def test_same_type_selects_complete_matching_eligible_squads(self):
        squads, groups, entities, registry = [], [], [], []
        for i in range(5):
            squad = self.facet(vt=self.squad_vt, enabled=i != 4)
            facets = [self.facet(squad) for _ in range(3)]
            members = [self.entity(f) for f in facets]
            entity = self.squad_entity(members, squad)
            if i == 2:  # different squad blueprint
                C.c_uint64.from_address(entity + 0x18).value = 3
            if i == 3:  # rejected by region/ownership filter
                C.c_ubyte.from_address(entity + 0x20).value = 0
            squads.append(squad)
            groups.append(facets)
            entities.append(entity)
            registry.extend([entity, *members])
        vector = self.vector(registry)
        manager = self.native.data(0x38, [
            (0x28, C.c_uint64.from_address(vector).value),
            (0x30, C.c_uint64.from_address(vector + 8).value)])
        self.type_stock(manager, None, entities[0])
        self.assertEqual([self.is_selected(f) for g in groups for f in g], [0] * 15)
        self.assertEqual([C.c_ubyte.from_address(s + 0x30).value for s in squads],
                         [1, 1, 0, 0, 0])
        self.clear_all(manager)
        self.type_patched(manager, None, entities[0])
        self.assertEqual([[self.is_selected(f) for f in g] for g in groups],
                         [[1, 1, 1], [1, 1, 1], [0, 0, 0], [0, 0, 0], [0, 0, 0]])

    def test_world_double_matches_soldiers_before_expanding_squads(self):
        groups, members_by_squad, registry = [], [], []
        for i in range(5):
            squad = self.facet(vt=self.squad_vt)
            facets = [self.facet(squad, enabled=i != 4) for _ in range(3)]
            members = [self.entity(f) for f in facets]
            entity = self.squad_entity(members, squad)
            holder = self.native.data(0x18, [(0x10, entity)])
            C.c_uint64.from_address(squad + 0x10).value = holder
            # Squad containers fail the world-object eligibility check. Only
            # one member of each matching squad is an eligible type match.
            C.c_ubyte.from_address(entity + 0x20).value = 0
            for j, member in enumerate(members):
                C.c_uint64.from_address(member + 0x18).value = (
                    1 if j == 0 and i != 2 else 3)
                if i == 3:
                    C.c_ubyte.from_address(member + 0x20).value = 0
            groups.append(facets)
            members_by_squad.append(members)
            # Include squad containers as well as members, in mixed order.
            registry.extend([members[0], entity, *members[1:]])
        vector = self.vector(registry)
        vt = self.native.data(0x88, [
            (0x60, C.cast(self.select_patched, C.c_void_p).value),
            (0x80, C.cast(self.type_patched, C.c_void_p).value)])
        manager = self.native.data(0x38, [(0, vt),
            (0x28, C.c_uint64.from_address(vector).value),
            (0x30, C.c_uint64.from_address(vector + 8).value)])
        # Reproduce the previous regression: a squad template accepts nothing.
        holder = C.c_uint64.from_address(
            C.c_uint64.from_address(groups[0][0] + 0x28).value + 0x10).value
        squad_entity = C.c_uint64.from_address(holder + 0x10).value
        self.type_patched(manager, None, squad_entity)
        self.assertEqual([self.is_selected(f) for g in groups for f in g], [0] * 15)
        self.world_double(manager, members_by_squad[0][0])
        self.assertEqual([[self.is_selected(f) for f in g] for g in groups],
                         [[1, 1, 1], [1, 1, 1], [0, 0, 0], [0, 0, 0], [0, 0, 0]])
        # No eligible match must not spuriously select anything.
        self.clear_all(manager)
        absent = self.entity(self.facet())
        C.c_uint64.from_address(absent + 0x18).value = 99
        self.world_double(manager, absent)
        self.assertEqual([self.is_selected(f) for g in groups for f in g], [0] * 15)

    def test_real_manager_reproduces_leader_only_then_selects_all(self):
        squad = self.facet(vt=self.squad_vt)
        facets = [self.facet(squad) for _ in range(3)]
        entity = self.squad_entity([self.entity(f) for f in facets], squad)
        self.select_stock(None, entity)
        self.assertEqual([self.is_selected(f) for f in facets], [1, 0, 0])
        for f in facets:
            self.set_selected(f, 0)
        self.select_patched(None, entity)
        self.assertEqual([self.is_selected(f) for f in facets], [1, 1, 1])

    def test_manager_individual_path_still_selects_only_target(self):
        squad = self.facet(vt=self.squad_vt)
        facets = [self.facet(squad) for _ in range(3)]
        self.select_patched(None, self.entity(facets[1]))
        self.assertEqual([self.is_selected(f) for f in facets], [0, 1, 0])

    def test_manager_squad_missing_members_and_empty_roster(self):
        squad = self.facet(vt=self.squad_vt)
        valid = self.facet(squad)
        entity = self.squad_entity([0, self.entity(0), self.entity(valid)], squad)
        self.select_patched(None, entity)
        self.assertEqual(self.is_selected(valid), 1)
        self.select_patched(None, self.squad_entity([], squad))

    def test_plain_squad_after_individual_selection_restores_all(self):
        squad = self.facet(vt=self.squad_vt)
        facets = [self.facet(squad) for _ in range(3)]
        members = [self.entity(f) for f in facets]
        entity = self.squad_entity(members, squad)
        self.select_patched(None, members[2])
        self.assertEqual(self.apply_filter(members), [members[2]])
        self.select_patched(None, entity)
        self.assertEqual(self.apply_filter(members), members)

    def apply_filter(self, entities, subset=True):
        begin = self.native.data((len(entities) + 1) * 8,
                                 [(i * 8, e) for i, e in enumerate(entities)]
                                 + [(len(entities) * 8, 0xfeedface)])
        end = begin + len(entities) * 8
        new_end = self.filter(begin, end, subset, self.soldier_vt)
        self.assertTrue(begin <= new_end <= end)
        self.assertEqual((new_end - begin) % 8, 0)
        self.assertEqual(C.c_uint64.from_address(end).value, 0xfeedface)
        return [C.c_uint64.from_address(a).value for a in range(begin, new_end, 8)]

    def test_multiple_marks_and_unmark_preserve_squad(self):
        squad = self.facet(vt=self.squad_vt)
        a, c = self.facet(squad), self.facet(squad)
        self.set_selected(a, 1)
        self.set_selected(c, 1)
        self.assertEqual((self.is_selected(a), self.is_selected(c)), (1, 1))
        self.set_selected(a, 0)
        self.assertEqual((self.is_selected(a), self.is_selected(c)), (0, 1))
        self.assertEqual(C.c_ubyte.from_address(squad + 0x30).value, 1)
        self.assertEqual(C.c_uint32.from_address(squad + 0x34).value, 2)

    def linked_squad(self, count):
        """A squad whose facet reaches its entity through the holder chain, so
        setSelected can list the members, selected as a whole."""
        squad = self.facet(vt=self.squad_vt)
        facets = [self.facet(squad) for _ in range(count)]
        entity = self.squad_entity([self.entity(f) for f in facets], squad)
        holder = self.native.data(0x18, [(0x10, entity)])
        C.c_uint64.from_address(squad + 0x10).value = holder
        self.select_patched(None, entity)
        return squad, facets

    def forwards(self, squad):
        return C.c_uint32.from_address(squad + 0x34).value

    def test_unmarking_the_last_soldier_deselects_the_squad(self):
        squad, facets = self.linked_squad(3)
        self.assertEqual([self.is_selected(f) for f in facets], [1, 1, 1])
        before = self.forwards(squad)
        self.set_selected(facets[0], 0)
        self.set_selected(facets[2], 0)
        # one soldier still marked: the squad stays, and was not told anything
        self.assertEqual(C.c_ubyte.from_address(squad + 0x30).value, 1)
        self.assertEqual(self.forwards(squad), before)
        self.assertEqual([self.is_selected(f) for f in facets], [0, 1, 0])
        self.set_selected(facets[1], 0)
        self.assertEqual(C.c_ubyte.from_address(squad + 0x30).value, 0)
        self.assertEqual(self.forwards(squad), before + 1)
        self.assertEqual([self.is_selected(f) for f in facets], [0, 0, 0])

    def test_a_disabled_mark_does_not_hold_the_squad(self):
        squad, facets = self.linked_squad(2)
        C.c_ubyte.from_address(facets[1] + 0x18).value = 0
        self.set_selected(facets[0], 0)
        self.assertEqual(C.c_ubyte.from_address(squad + 0x30).value, 0)

    def test_marking_into_a_deselected_squad_drops_stale_marks(self):
        squad, facets = self.linked_squad(3)
        # deselect(squad) writes only the squad's flag, as the manager does
        C.c_ubyte.from_address(squad + 0x30).value = 0
        self.assertEqual([C.c_ubyte.from_address(f + 0x30).value for f in facets], [1, 1, 1])
        self.set_selected(facets[1], 1)
        self.assertEqual([C.c_ubyte.from_address(f + 0x30).value for f in facets], [0, 1, 0])
        self.assertEqual(C.c_ubyte.from_address(squad + 0x30).value, 1)
        self.assertEqual([self.is_selected(f) for f in facets], [0, 1, 0])

    def test_unmarking_in_a_deselected_squad_leaves_it_alone(self):
        squad, facets = self.linked_squad(2)
        C.c_ubyte.from_address(squad + 0x30).value = 0
        before = self.forwards(squad)
        self.set_selected(facets[0], 0)
        self.assertEqual(C.c_ubyte.from_address(facets[0] + 0x30).value, 0)
        self.assertEqual(C.c_ubyte.from_address(facets[1] + 0x30).value, 1)
        self.assertEqual(self.forwards(squad), before)

    def test_soldier_without_squad(self):
        a = self.facet()
        for value in (1, 0, 1):
            self.set_selected(a, value)
            self.assertEqual(self.is_selected(a), value)

    def test_disabled_mark_is_not_selected(self):
        a = self.facet(enabled=False)
        self.set_selected(a, 1)
        self.assertEqual(self.is_selected(a), 0)

    def test_direct_squad_deselect_hides_mark(self):
        # A direct squad-flag write is not the manager's ordinary-click clear.
        squad = self.facet(vt=self.squad_vt)
        a = self.facet(squad)
        self.set_selected(a, 1)
        C.c_ubyte.from_address(squad + 0x30).value = 0
        self.assertEqual(self.is_selected(a), 0)
        self.assertEqual(self.apply_filter([self.entity(a)]), [])
        # The parent guard masks the mark; it does not clear persisted state.
        self.assertEqual(C.c_ubyte.from_address(a + 0x30).value, 1)

    def test_manager_clear_removes_marks_across_squads(self):
        squads = [self.facet(vt=self.squad_vt, marked=True) for _ in range(2)]
        soldiers = [self.facet(squads[0]), self.facet(squads[0]),
                    self.facet(squads[1])]
        for soldier in soldiers:
            self.set_selected(soldier, 1)
        # Interleave squads and soldiers: cleanup must not depend on order.
        facets = [soldiers[0], squads[1], soldiers[2], squads[0], soldiers[1]]
        entities = [self.entity(f) for f in facets]
        vector = self.native.data(len(entities) * 8,
                                  [(i * 8, e) for i, e in enumerate(entities)])
        manager = self.native.data(0x38, [(0x28, vector),
                                         (0x30, vector + len(entities) * 8)])
        self.clear_all(manager)
        for facet in facets:
            self.assertEqual(C.c_ubyte.from_address(facet + 0x30).value, 0)
        for soldier in soldiers:
            self.assertEqual(self.is_selected(soldier), 0)

    def test_manager_clear_empty_registry(self):
        manager = self.native.data(0x38)
        self.clear_all(manager)

    def test_filter_squad_mode_preserves_every_entry(self):
        items = [0, self.entity(self.facet()), self.entity(self.facet(marked=True))]
        self.assertEqual(self.apply_filter(items, False), items)

    def test_filter_empty_and_unmarked_subsets(self):
        self.assertEqual(self.apply_filter([]), [])
        self.assertEqual(self.apply_filter([self.entity(self.facet())]), [])

    def test_filter_keeps_order_across_squads(self):
        squads = [self.facet(vt=self.squad_vt, marked=True) for _ in range(2)]
        a = self.entity(self.facet(squads[0], marked=True))
        c = self.entity(self.facet(squads[1], marked=True))
        ignored = self.entity(self.facet(squads[0]))
        self.assertEqual(self.apply_filter([a, ignored, c]), [a, c])

    def test_filter_rejects_missing_wrong_and_disabled_facets(self):
        no_container = self.native.data(0x10, [(0, self.entity_vt)])
        items = [0, no_container, self.entity(0),
                 self.entity(self.facet(enabled=False, marked=True)),
                 self.entity(self.facet(marked=True, vt=self.squad_vt))]
        self.assertEqual(self.apply_filter(items), [])


if __name__ == "__main__":
    unittest.main()
