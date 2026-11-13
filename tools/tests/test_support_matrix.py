"""Tests for the program table of tools/generate_support_matrix.py (run with:
python3 -m unittest discover -s tools/tests)."""

import sys
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOLS))

import generate_support_matrix as gsm  # noqa: E402

INDEX = [
    {"id": "a-bound", "layers": ["Kitty"], "environment": "terminals"},
    {"id": "b-bound", "layers": ["Xfce"], "environment": "xfce"},
]


def entry(identifier, lab=(), binding=(), tolerated=()):
    row = {"id": identifier, "lab": list(lab)}
    if binding:
        row["lab_binding"] = list(binding)
    if tolerated:
        row["lab_tolerated"] = list(tolerated)
    return row


class ProgramStatus(unittest.TestCase):
    def status(self, program, entries):
        return gsm.program_status(program, entries, INDEX)[0]

    def test_a_scenario_in_the_entries_lab_list_verifies_the_program(self):
        program = {"name": "kitty", "entries": ["t"], "layer": "Kitty"}
        self.assertEqual(self.status(program, [entry("t", lab=["a-bound"])]), gsm.VERIFIED)

    def test_a_passing_scenario_of_another_program_does_not(self):
        program = {"name": "foot", "entries": ["t"], "layer": "Foot"}
        self.assertEqual(self.status(program, [entry("t", lab=["a-bound"])]), gsm.UNVERIFIED)

    def test_binding_only_evidence_is_partial(self):
        program = {"name": "Xfce", "entries": ["x"], "layer": "Xfce"}
        self.assertEqual(self.status(program, [entry("x", binding=["b-bound"])]), gsm.PARTIAL)

    def test_a_live_note_alone_is_partial_and_nothing_is_unverified(self):
        entries = [entry("x")]
        live = {"name": "Neovim", "entries": ["x"], "live": "by hand"}
        self.assertEqual(self.status(live, entries), gsm.PARTIAL)
        self.assertEqual(self.status({"name": "River", "entries": ["x"]}, entries), gsm.UNVERIFIED)

    def test_a_tolerated_scenario_or_a_partial_note_keeps_it_from_verified(self):
        program = {"name": "kitty", "entries": ["t"], "layer": "Kitty"}
        both = [entry("t", lab=["a-bound"], tolerated=["a-bound2"])]
        index = INDEX + [{"id": "a-bound2", "layers": ["Kitty"], "environment": "terminals"}]
        self.assertEqual(gsm.program_status(program, both, index)[0], gsm.PARTIAL)
        self.assertEqual(
            gsm.program_status({**program, "partial": "x"}, [entry("t", lab=["a-bound"])], INDEX)[0],
            gsm.PARTIAL,
        )

    def test_only_tolerated_scenarios_is_partial(self):
        program = {"name": "kitty", "entries": ["t"], "layer": "Kitty"}
        self.assertEqual(self.status(program, [entry("t", tolerated=["a-bound"])]), gsm.PARTIAL)

    def test_fold_records_exact_and_tolerated_passes_apart(self):
        entries = [entry("editor.terminal-apps")]
        records = [
            {"scenario": "vim-a", "pass": True, "tolerated": False, "layer": "Vim"},
            {"scenario": "vim-b", "pass": True, "tolerated": True, "layer": "Vim"},
        ]
        real = gsm.scenario_ids
        gsm.scenario_ids = lambda: {"vim-a", "vim-b"}
        try:
            gsm.fold_results(entries, records)
        finally:
            gsm.scenario_ids = real
        self.assertEqual(entries[0]["lab"], ["vim-a"])
        self.assertEqual(entries[0]["lab_tolerated"], ["vim-b"])

    def test_every_program_of_the_manifest_has_a_symbol_and_the_readme_matches(self):
        document = gsm.load_document()
        rows = gsm.program_rows(document)
        self.assertTrue(all(row[1] in (gsm.VERIFIED, gsm.PARTIAL, gsm.UNVERIFIED) for row in rows))
        self.assertEqual(gsm.README.read_text(encoding="utf-8"), gsm.spliced_readme(document))


if __name__ == "__main__":
    unittest.main()
