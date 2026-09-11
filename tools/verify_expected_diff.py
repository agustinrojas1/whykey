#!/usr/bin/env python3
"""tools/verify_expected_diff.py

Strict, syntax-aware JSON differential verifier for Whykey regression comparisons.
Validates that a candidate JSON document differs from a baseline document ONLY
by an approved, scenario-specific typed extension at the exact designated path.

Guarantees:
- Rejects duplicate object keys in baseline or candidate JSON (no silent key shadowing).
- Enforces exact JSON path and exact value match for the approved addition.
- Performs deep equality comparison on the remaining document AST.
- Rejects any unexpected modifications, additions, or deletions elsewhere.
"""

import json
import sys


def parse_no_duplicates(pairs):
    """JSON object hook that strictly rejects duplicate keys."""
    d = {}
    for k, v in pairs:
        if k in d:
            raise ValueError(f"Duplicate key detected in JSON object: {k!r}")
        d[k] = v
    return d


def load_strict_json(file_path):
    with open(file_path, "r", encoding="utf-8") as f:
        content = f.read()
    return json.loads(content, object_pairs_hook=parse_no_duplicates)


def verify_json_v2_modifier_ambiguity(base_path, cand_path):
    try:
        base = load_strict_json(base_path)
    except Exception as e:
        sys.stderr.write(f"Error parsing baseline JSON '{base_path}': {e}\n")
        return 1

    try:
        cand = load_strict_json(cand_path)
    except Exception as e:
        sys.stderr.write(f"Error parsing candidate JSON '{cand_path}': {e}\n")
        return 1

    # 1. Verify schema version is explicitly 2 in both documents
    if cand.get("schema_version") != 2 or base.get("schema_version") != 2:
        sys.stderr.write(
            f"Verification failed: expected schema_version == 2 in both documents, got base={base.get('schema_version')!r}, cand={cand.get('schema_version')!r}\n"
        )
        return 1

    # 2. In schema v2 inspect output, layers are in the 'path' array.
    # Locate and verify the Hyprland layer (expected at index 0).
    try:
        cand_layers = cand.get("path")
        base_layers = base.get("path")
        if not isinstance(cand_layers, list) or not isinstance(base_layers, list) or not cand_layers or not base_layers:
            sys.stderr.write("Verification failed: non-empty 'path' array missing in JSON v2 report\n")
            return 1

        cand_layer_name = cand_layers[0].get("layer")
        base_layer_name = base_layers[0].get("layer")
        if cand_layer_name != "Hyprland" or base_layer_name != "Hyprland":
            sys.stderr.write(
                f"Verification failed: expected path[0].layer == 'Hyprland', got base={base_layer_name!r}, cand={cand_layer_name!r}\n"
            )
            return 1

        cand_binding = cand_layers[0].get("binding")
        base_binding = base_layers[0].get("binding")
        if not isinstance(cand_binding, dict) or not isinstance(base_binding, dict):
            sys.stderr.write("Verification failed: path[0].binding object missing\n")
            return 1
    except (KeyError, IndexError, TypeError) as e:
        sys.stderr.write(f"Verification failed: structure error: {e}\n")
        return 1
    # Exact value check
    cand_uncertainty = cand_binding.get("uncertainty")
    if cand_uncertainty != "ModifierAmbiguity":
        sys.stderr.write(
            f"Verification failed: expected path[0].binding.uncertainty == 'ModifierAmbiguity', got {cand_uncertainty!r}\n"
        )
        return 1

    # Verify baseline did not have this field
    base_uncertainty = base_binding.get("uncertainty")
    if base_uncertainty is not None:
        sys.stderr.write(
            f"Verification failed: baseline already has uncertainty: {base_uncertainty!r}\n"
        )
        return 1

    # Remove the single approved field and verify deep structural equality
    del cand_binding["uncertainty"]
    if cand != base:
        sys.stderr.write(
            "Verification failed: candidate has unexpected differences beyond path[0].binding.uncertainty\n"
        )
        return 1

    return 0


def main():
    if len(sys.argv) != 4:
        sys.stderr.write(
            f"Usage: {sys.argv[0]} <rule_name> <baseline_json> <candidate_json>\n"
        )
        sys.exit(2)

    rule = sys.argv[1]
    base_file = sys.argv[2]
    cand_file = sys.argv[3]

    if rule == "json_v2_modifier_ambiguity":
        sys.exit(verify_json_v2_modifier_ambiguity(base_file, cand_file))
    else:
        sys.stderr.write(f"Unknown expected difference rule: '{rule}'\n")
        sys.exit(2)


if __name__ == "__main__":
    main()
