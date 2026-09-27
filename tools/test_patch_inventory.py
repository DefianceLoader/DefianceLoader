"""tools/patch_inventory.py's rules on synthetic runs: overlaps between
plugins' solo writes, and plugins that install differently when started with
every other plugin. Needs no DLLs."""
import pathlib, sys, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import patch_inventory


def result(order, spans, states=None):
    """A run of `order` in which `spans` ({id: [(module, rva, length)]}) are
    written; every plugin is Active unless `states` says otherwise."""
    return {"states": {id: (states or {}).get(id, "Active") for id in order}, "order": order,
            "spans": [{"plugin": id, "module": m, "rva": rva, "length": n, "kind": "function"}
                      for id, written in spans.items() for m, rva, n in written]}


def compare(spans, forward=None, reverse=None, ordered=None, states=None):
    """compare()'s problems for plugins `spans` writing the same alone as
    together, unless the forward/reverse runs are given."""
    ids = sorted(spans)
    solos = {id: result([id], {id: spans[id]}, states) for id in ids}
    forward = forward or result(ids, spans, states)
    reverse = reverse or result(ids[::-1], spans, states)
    return patch_inventory.compare(ids, solos, forward, reverse, ordered or {})[1]


class Rules(unittest.TestCase):
    def test_disjoint_writes_pass(self):
        self.assertEqual(compare({"a": [("game", 0x10, 5)], "b": [("game", 0x15, 5)]}), [])

    def test_a_partial_overlap_is_found_whatever_the_starts(self):
        problems = compare({"a": [("game", 0x10, 14)], "b": [("game", 0x18, 5)]})
        self.assertEqual(len(problems), 1)
        self.assertIn("(a, function) overlaps 0x18..0x1d (b, function)", problems[0])

    def test_the_same_address_in_another_module_is_no_overlap(self):
        self.assertEqual(compare({"a": [("game", 0x10, 5)], "b": [("logic", 0x10, 5)]}), [])

    def test_one_plugins_own_adjacent_spans_are_no_overlap(self):
        self.assertEqual(compare({"a": [("game", 0x10, 5), ("game", 0x12, 5)]}), [])

    def test_a_plugin_that_fails_after_another_is_reported(self):
        spans = {"a": [("game", 0x10, 5)], "b": [("game", 0x40, 5)]}
        reverse = result(["b", "a"], {"b": spans["b"]}, {"a": "Failed"})
        problems = compare(spans, reverse=reverse)
        self.assertEqual(len(problems), 1)
        self.assertIn("a installs differently with every plugin (reverse order): Failed", problems[0])

    def test_other_spans_together_are_reported(self):
        spans = {"a": [("game", 0x10, 5)], "b": [("game", 0x40, 5)]}
        forward = result(["a", "b"], {"a": spans["a"], "b": [("game", 0x60, 5)]})
        self.assertIn("b installs differently", compare(spans, forward=forward)[0])

    def test_a_declared_order_allows_the_difference_only_when_reversed(self):
        spans = {"a": [("game", 0x10, 5)], "b": [("game", 0x40, 5)]}
        ordered = {("a", "b"): "b hooks bytes a checks"}
        reverse = result(["b", "a"], {"b": spans["b"]}, {"a": "Failed"})
        self.assertEqual(compare(spans, reverse=reverse, ordered=ordered), [])
        # the same failure where the loader keeps the declared order is not allowed
        forward = result(["a", "b"], {"b": spans["b"]}, {"a": "Failed"})
        self.assertEqual(len(compare(spans, forward=forward, ordered=ordered)), 1)

    def test_the_loader_must_keep_a_declared_order(self):
        spans = {"a": [("game", 0x10, 5)], "b": [("game", 0x40, 5)]}
        forward = result(["b", "a"], spans)
        problems = compare(spans, forward=forward, ordered={("a", "b"): "b hooks bytes a checks"})
        self.assertEqual(problems, ["the loader starts b before a, but b hooks bytes a checks"])

    def test_writes_outside_the_modules_are_reported(self):
        self.assertIn("outside the mapped modules", compare({"a": [("other", 0x7ff00000, 5)]})[0])


class Fixture(unittest.TestCase):
    def test_writes_in_a_module_the_build_lacks_are_not_compared(self):
        recorded = {"plugins": {"a": {"state": "Active", "spans": [["game", 16, 5, "function"],
                                                                 ["world2", 32, 5, "function"]]}}}
        record = {"plugins": {"a": {"state": "Active", "spans": [["game", 16, 5, "function"]]}}}
        self.assertEqual(patch_inventory.differences(recorded, record, ["world2"]), [])
        self.assertEqual(patch_inventory.differences(recorded, record),
                         ["a: no longer writes ['world2', 32, 5, 'function']"])


if __name__ == "__main__":
    unittest.main()
