#!/usr/bin/env python3
"""Hermetic test runner for first-party whykey extensions."""

import os
import subprocess
import sys


def main():
    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
    extensions = [
        os.path.join(repo_root, "extensions", "whykey-nvim"),
        os.path.join(repo_root, "extensions", "whykey-vscode"),
        os.path.join(repo_root, "extensions", "whykey-emacs"),
    ]

    failed = []
    for ext in extensions:
        name = os.path.basename(ext)
        print(f"Running {name} --test ... ", end="", flush=True)
        res = subprocess.run([sys.executable, ext, "--test"], capture_output=True, text=True)
        if res.returncode == 0:
            print("OK")
        else:
            print(f"FAILED (exit {res.returncode})")
            if res.stderr:
                print(res.stderr.strip())
            failed.append(name)

    if failed:
        print(f"\n{len(failed)} extension test(s) failed: {', '.join(failed)}")
        sys.exit(1)

    print(f"\nAll {len(extensions)} first-party extensions passed self-tests.")


if __name__ == "__main__":
    main()
