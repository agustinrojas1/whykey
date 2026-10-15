"""Tests for tools/lab_select.py (run with: python3 -m unittest discover -s tools/tests).

The expected scenario sets come from the real lab/scenarios tree, so the tests
check the rules, not a frozen list of ids.
"""

import fnmatch
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOLS))

import lab_images  # noqa: E402
import lab_select  # noqa: E402

SCENARIOS = lab_select.load_scenarios()
TOTAL = len(SCENARIOS)


def ids_of(image=None, prefix=None, environment=None):
    return {
        s.id for s in SCENARIOS
        if (image is None or s.image == image)
        and (prefix is None or s.rel.startswith(prefix))
        and (environment is None or s.environment == environment)
    }


def selected_ids(plan):
    """The scenario ids a plan runs, recomputed the way lab/run.py selects them:
    --group IMAGE, then --only globs (id or lab/scenarios-relative path), then
    --shard I/N over the sorted ids."""
    run = set()
    for shard in plan["shards"]:
        members = sorted((s for s in SCENARIOS if s.image == shard["image"]), key=lambda s: s.id)
        if shard["only"] is not None:
            members = [s for s in members if any(
                fnmatch.fnmatch(s.id, pattern) or fnmatch.fnmatch(s.rel, pattern)
                for pattern in shard["only"])]
        index, count = (int(x) for x in shard["shard"].split("/"))
        run.update(s.id for s in lab_select.shard_slice(members, index, count))
    return run


def images_selected(plan):
    return {name for name, info in plan["images"].items() if info["selected"]}


class GlobTest(unittest.TestCase):
    def test_star_stays_in_one_segment(self):
        pattern = lab_select.glob_regex("*.md")
        self.assertTrue(pattern.match("README.md"))
        self.assertFalse(pattern.match("docs/guide.md"))

    def test_double_star_crosses_segments(self):
        pattern = lab_select.glob_regex("**/*.md")
        self.assertTrue(pattern.match("README.md"))
        self.assertTrue(pattern.match("lab/README.md"))
        self.assertFalse(pattern.match("lab/README.txt"))

    def test_src_double_star(self):
        pattern = lab_select.glob_regex("src/layers/shell/**")
        self.assertTrue(pattern.match("src/layers/shell/fish.rs"))
        self.assertFalse(pattern.match("src/layers/shell.rs"))


class ShardTest(unittest.TestCase):
    def test_slices_partition_the_list(self):
        items = list(range(23))
        for count in range(1, 6):
            parts = [lab_select.shard_slice(items, index, count) for index in range(1, count + 1)]
            self.assertEqual(sorted(sum(parts, [])), items)
            self.assertLessEqual(max(map(len, parts)) - min(map(len, parts)), 1)

    def test_parse_shard(self):
        self.assertEqual(lab_select.parse_shard("2/3"), (2, 3))
        for bad in ("0/3", "4/3", "3", "a/b", "1/0"):
            with self.assertRaises(ValueError, msg=bad):
                lab_select.parse_shard(bad)


class SelectionTest(unittest.TestCase):
    def test_readme_only_selects_nothing(self):
        plan = lab_select.make_plan(["README.md"], False)
        self.assertFalse(plan["full"])
        self.assertEqual(plan["selected"], 0)
        self.assertEqual(plan["shards"], [])

    def test_docs_packaging_and_workflows_select_nothing(self):
        plan = lab_select.make_plan(
            ["docs/guide.md", "packaging/whykey.spec", ".github/workflows/ci.yml",
             ".github/workflows/release.yml", "lab/README.md", "CHANGELOG.md", "tests/cli.rs"],
            False)
        self.assertFalse(plan["full"])
        self.assertEqual(plan["shards"], [])

    def test_zellij_layer_selects_only_zellij(self):
        plan = lab_select.make_plan(["src/layers/zellij.rs"], False)
        self.assertFalse(plan["full"])
        self.assertEqual(images_selected(plan), {"tty"})
        self.assertEqual(selected_ids(plan), ids_of(prefix="termapps/zellij-"))
        self.assertTrue(selected_ids(plan))

    def test_report_selects_everything(self):
        plan = lab_select.make_plan(["src/report.rs"], False)
        self.assertTrue(plan["full"])
        self.assertEqual(plan["selected"], TOTAL)
        self.assertEqual(selected_ids(plan), {s.id for s in SCENARIOS})

    def test_unknown_source_file_selects_everything(self):
        plan = lab_select.make_plan(["src/brand_new_module.rs"], False)
        self.assertTrue(plan["full"])
        self.assertEqual(plan["selected"], TOTAL)

    def test_lab_run_script_selects_everything(self):
        self.assertTrue(lab_select.make_plan(["lab/run.py"], False)["full"])

    def test_package_family_selects_its_image(self):
        plan = lab_select.make_plan(["lab/packages/remap.txt"], False)
        self.assertEqual(images_selected(plan), {"vm"})
        self.assertEqual(selected_ids(plan), ids_of(image="vm"))
        self.assertIsNone(plan["shards"][0]["only"])

    def test_base_family_selects_everything(self):
        plan = lab_select.make_plan(["lab/packages/base.txt"], False)
        self.assertEqual(plan["selected"], TOTAL)
        self.assertEqual(selected_ids(plan), {s.id for s in SCENARIOS})

    def test_image_dockerfile_selects_its_image(self):
        plan = lab_select.make_plan(["lab/images/wayland.Dockerfile"], False)
        self.assertEqual(images_selected(plan), {"wayland"})
        self.assertEqual(selected_ids(plan), ids_of(image="wayland"))

    def test_scenario_file_selects_that_scenario(self):
        plan = lab_select.make_plan(["lab/scenarios/sway/sway-plain-bind.json"], False)
        self.assertEqual(selected_ids(plan), {"sway-plain-bind"})
        self.assertEqual(plan["shards"][0]["only"], ["sway/sway-plain-bind.json"])

    def test_deleted_scenario_file_selects_nothing(self):
        plan = lab_select.make_plan(["lab/scenarios/sway/no-such-scenario.json"], False)
        self.assertEqual(plan["shards"], [])

    def test_environment_plugin_selects_its_scenarios(self):
        plan = lab_select.make_plan(["lab/envs/sway.py"], False)
        self.assertEqual(selected_ids(plan), ids_of(environment="sway"))

    def test_mixed_paths_union(self):
        plan = lab_select.make_plan(["README.md", "src/layers/zellij.rs"], False)
        self.assertEqual(selected_ids(plan), ids_of(prefix="termapps/zellij-"))

    def test_unmapped_file_wins_over_mapped_ones(self):
        plan = lab_select.make_plan(["src/layers/zellij.rs", "src/unknown.rs"], False)
        self.assertTrue(plan["full"])

    def test_herbstluftwm_has_no_scenarios(self):
        plan = lab_select.make_plan(["src/layers/herbstluftwm.rs"], False)
        self.assertFalse(plan["full"])
        self.assertEqual(plan["shards"], [])

    def test_full_run_shards(self):
        plan = lab_select.make_plan(None, True)
        self.assertTrue(plan["full"])
        self.assertEqual(plan["selected"], TOTAL)
        counts = {}
        for shard in plan["shards"]:
            self.assertIsNone(shard["only"])
            counts[shard["image"]] = counts.get(shard["image"], 0) + 1
        self.assertEqual(counts["vm"], 3)
        self.assertEqual(counts["tty"], 2)
        self.assertEqual(counts["wayland"], 1)
        self.assertEqual(counts["x11"], 1)
        self.assertEqual(counts["gnome"], 1)
        self.assertEqual(sum(shard["count"] for shard in plan["shards"]), TOTAL)
        self.assertEqual(selected_ids(plan), {s.id for s in SCENARIOS})
        for shard in plan["shards"]:
            self.assertEqual(shard["args"][:2], ["--group", shard["image"]])
            self.assertIn("--shard", shard["args"])

    def test_shards_are_disjoint(self):
        plan = lab_select.make_plan(None, True)
        for image in {shard["image"] for shard in plan["shards"]}:
            members = sorted(s.id for s in SCENARIOS if s.image == image)
            seen = []
            for shard in (s for s in plan["shards"] if s["image"] == image):
                index, count = (int(x) for x in shard["shard"].split("/"))
                seen += [s.id for s in lab_select.shard_slice(
                    [s for s in sorted(SCENARIOS, key=lambda s: s.id) if s.image == image],
                    index, count)]
            self.assertEqual(sorted(seen), members)

    def test_vm_shards_are_small(self):
        plan = lab_select.make_plan(None, True)
        for shard in plan["shards"]:
            limit = 22 if shard["image"] == "vm" else 60
            self.assertLessEqual(shard["count"], limit, shard["name"])

    def test_partial_vm_selection_is_one_shard(self):
        plan = lab_select.make_plan(["lab/scenarios/remap/remap-keyd-redirect-bound.json"], False)
        self.assertEqual([shard["name"] for shard in plan["shards"]], ["vm-1of1"])
        self.assertEqual(plan["shards"][0]["only"], ["remap/remap-keyd-redirect-bound.json"])

    def test_github_output(self):
        plan = lab_select.make_plan(["src/layers/zellij.rs"], False)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "github-output"
            summary = Path(directory) / "summary.md"
            lab_select.write_outputs(plan, output, summary)
            lines = output.read_text(encoding="utf-8").splitlines()
            self.assertIn("count=1", lines)
            self.assertIn("full=false", lines)
            self.assertTrue(any(line.startswith("shards=[{") for line in lines))
            self.assertIn("## Lab scenario selection", summary.read_text(encoding="utf-8"))

    def test_config_is_valid(self):
        # Every glob in lab/affected.json matches a scenario, and images are known.
        config = lab_select.load_config(SCENARIOS)
        self.assertTrue(config.rules)
        self.assertEqual(sorted(lab_images.load_manifest()["images"]),
                         sorted({s.image for s in SCENARIOS}))


if __name__ == "__main__":
    unittest.main()
