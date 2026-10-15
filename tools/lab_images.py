#!/usr/bin/env python3
"""Build and address the whykey lab images (stdlib only).

The lab runs in one shared base image (lab/images/base.Dockerfile) and a few
group images built FROM it (lab/images/<name>.Dockerfile), described by
lab/images.json. Every environment lab/envs/<name>.py belongs to exactly one
group image, and a scenario may name another image with a top-level "image"
field. An image's content hash covers its Dockerfile, lab/packages/_install.sh,
its families' .txt/.sh files and, for a group, the base image's hash, so a
package change rebuilds only the images that contain that family.

    python3 tools/lab_images.py list
    python3 tools/lab_images.py hash NAME
    python3 tools/lab_images.py build [--prefix P] [NAME...]
    python3 tools/lab_images.py tag-of NAME [--prefix P]

build tags <P><name>:<hash> and <P><name>:latest (P defaults to
"whykey-lab-") and passes --build-arg BASE=<P>base:<base hash> to the groups.
A group named without the base builds the base first (a cache hit when it is
current).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LAB = ROOT / "lab"
MANIFEST = LAB / "images.json"
IMAGES = LAB / "images"
PACKAGES = LAB / "packages"
ENVS = LAB / "envs"
BASE = "base"
DEFAULT_PREFIX = "whykey-lab-"


class ImageError(Exception):
    """lab/images.json or the image files are inconsistent."""


def environment_names() -> list[str]:
    """Every environment plugin: lab/envs/<name>.py, minus "_" helpers."""
    return sorted(path.stem for path in ENVS.glob("*.py") if not path.stem.startswith("_"))


def load_manifest(path: Path = MANIFEST) -> dict:
    """Parse and validate lab/images.json. Returns
    {"base": [families], "images": {name: {"families", "environments"}}}."""
    data = json.loads(path.read_text(encoding="utf-8"))
    base = data.get("base", {}).get("families")
    images = data.get("images")
    if not isinstance(base, list) or not base:
        raise ImageError('lab/images.json: "base" needs a non-empty "families" list')
    if not isinstance(images, dict) or not images:
        raise ImageError('lab/images.json: "images" needs at least one group')
    if BASE in images:
        raise ImageError(f'lab/images.json: "{BASE}" is the base image, not a group')
    seen_families: dict[str, str] = {family: BASE for family in base}
    if len(seen_families) != len(base):
        raise ImageError('lab/images.json: "base" lists a family twice')
    seen_envs: dict[str, str] = {}
    for name, spec in images.items():
        families = spec.get("families")
        environments = spec.get("environments", [])
        if not isinstance(families, list) or not isinstance(environments, list):
            raise ImageError(f'lab/images.json: image "{name}" needs "families" and "environments" lists')
        for family in families:
            if family in seen_families:
                raise ImageError(
                    f'lab/images.json: family "{family}" is in both "{seen_families[family]}" and "{name}"; '
                    "a family belongs to one image"
                )
            seen_families[family] = name
        for environment in environments:
            if environment in seen_envs:
                raise ImageError(
                    f'lab/images.json: environment "{environment}" is in both "{seen_envs[environment]}" and "{name}"'
                )
            seen_envs[environment] = name
    for family in seen_families:
        if not (PACKAGES / f"{family}.txt").is_file():
            raise ImageError(f"lab/packages/{family}.txt does not exist")
    for name in [BASE, *images]:
        if not (IMAGES / f"{name}.Dockerfile").is_file():
            raise ImageError(f"lab/images/{name}.Dockerfile does not exist")
    known = set(environment_names())
    unknown = sorted(set(seen_envs) - known)
    if unknown:
        raise ImageError(f"lab/images.json names unknown environments: {unknown}")
    missing = sorted(known - set(seen_envs))
    if missing:
        raise ImageError(f"lab/images.json: environments in no image: {missing}")
    return {
        "base": base,
        "images": {name: {"families": spec["families"], "environments": spec.get("environments", [])}
                   for name, spec in images.items()},
    }


def image_names(manifest: dict) -> list[str]:
    """All image names in build order: the base first, then the groups."""
    return [BASE, *manifest["images"]]


def families_of(manifest: dict, name: str) -> list[str]:
    return manifest["base"] if name == BASE else manifest["images"][name]["families"]


def environment_images(manifest: dict) -> dict[str, str]:
    """Environment name -> the group image it runs in."""
    return {env: name for name, spec in manifest["images"].items() for env in spec["environments"]}


def _check_name(manifest: dict, name: str) -> None:
    if name not in image_names(manifest):
        raise ImageError(f"unknown image {name!r}; known: {', '.join(image_names(manifest))}")


def content_hash(name: str, manifest: dict | None = None) -> str:
    """16 hex chars: sha256 over the image's Dockerfile, _install.sh, its
    families' .txt/.sh files and (for a group) the base image's hash."""
    manifest = manifest or load_manifest()
    _check_name(manifest, name)
    digest = hashlib.sha256()

    def feed(label: str, data: bytes) -> None:
        digest.update(label.encode("utf-8") + b"\0" + str(len(data)).encode() + b"\0" + data)

    feed(f"{name}.Dockerfile", (IMAGES / f"{name}.Dockerfile").read_bytes())
    feed("_install.sh", (PACKAGES / "_install.sh").read_bytes())
    for family in families_of(manifest, name):
        for suffix in ("txt", "sh"):
            path = PACKAGES / f"{family}.{suffix}"
            if path.is_file():
                feed(path.name, path.read_bytes())
    if name != BASE:
        feed("base", content_hash(BASE, manifest).encode())
    return digest.hexdigest()[:16]


def tag_of(name: str, prefix: str = DEFAULT_PREFIX, manifest: dict | None = None) -> str:
    """The full hash tag: <prefix><name>:<content hash>."""
    return f"{prefix}{name}:{content_hash(name, manifest)}"


def build(names: list[str], prefix: str, manifest: dict) -> None:
    order = image_names(manifest)
    for name in names:
        _check_name(manifest, name)
    selected = set(names) if names else set(order)
    if selected - {BASE}:
        selected.add(BASE)  # every group builds FROM the base image
    base_hash = content_hash(BASE, manifest)
    for name in order:
        if name not in selected:
            continue
        tag = f"{prefix}{name}:{content_hash(name, manifest)}"
        command = [
            "docker", "build",
            "-f", str(IMAGES / f"{name}.Dockerfile"),
            "-t", tag,
            "-t", f"{prefix}{name}:latest",
        ]
        if name != BASE:
            command += ["--build-arg", f"BASE={prefix}{BASE}:{base_hash}"]
        command.append(str(LAB))
        print(f"== {name}: {tag}", flush=True)
        subprocess.run(command, check=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list", help="image names in build order, base first")
    hash_parser = commands.add_parser("hash", help="print an image's content hash (16 hex chars)")
    hash_parser.add_argument("name")
    build_parser = commands.add_parser("build", help="build images locally with docker")
    build_parser.add_argument("--prefix", default=DEFAULT_PREFIX, help=f"image name prefix (default {DEFAULT_PREFIX!r})")
    build_parser.add_argument("names", nargs="*", metavar="NAME", help="images to build (default: all)")
    tag_parser = commands.add_parser("tag-of", help="print an image's full hash tag")
    tag_parser.add_argument("name")
    tag_parser.add_argument("--prefix", default=DEFAULT_PREFIX)
    args = parser.parse_args(argv)
    try:
        manifest = load_manifest()
        if args.command == "list":
            print("\n".join(image_names(manifest)))
        elif args.command == "hash":
            print(content_hash(args.name, manifest))
        elif args.command == "tag-of":
            print(tag_of(args.name, args.prefix, manifest))
        elif args.command == "build":
            build(args.names, args.prefix, manifest)
    except ImageError as error:
        print(f"lab_images: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
