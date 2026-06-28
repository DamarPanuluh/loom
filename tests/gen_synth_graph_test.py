#!/usr/bin/env python3
"""Regression tests for gen_synth_graph.py edge builders."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / ".claude" / "skills" / "run-loom" / "gen_synth_graph.py"


def _generate_graph():
    tmp = tempfile.mkdtemp()
    out = Path(tmp) / "synth.graph.json"
    subprocess.check_call([sys.executable, str(SCRIPT), str(out), "50", "100"])
    return json.loads(out.read_text())


class GenSynthGraphTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.graph = _generate_graph()

    def test_emits_hierarchy_edges(self):
        self.assertGreater(len(self.graph["edges"].get("HIERARCHY", [])), 0)

    def test_emits_implements_edges(self):
        self.assertGreater(len(self.graph["edges"].get("IMPLEMENTS", [])), 0)


if __name__ == "__main__":
    unittest.main()
