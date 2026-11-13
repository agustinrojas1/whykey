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


def entry(identifier, lab=(), binding=()):
    row = {"id": identifier, "lab": list(lab)}
    if binding:
        row["lab_binding"] = list(binding)
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

    def test_every_program_of_the_manifest_has_a_symbol_and_the_readme_matches(self):
        document = gsm.load_document()
        rows = gsm.program_rows(document)
        self.assertTrue(all(symbol in (gsm.VERIFIED, gsm.PARTIAL, gsm.UNVERIFIED) for _, symbol, _, _ in rows))
        self.assertEqual(gsm.README.read_text(encoding="utf-8"), gsm.spliced_readme(document))


if __name__ == "__main__":
    unittest.main()
