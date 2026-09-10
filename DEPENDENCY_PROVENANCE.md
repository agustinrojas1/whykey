# Dependency provenance and tested versions

This file is generated from `Cargo.toml` and `Cargo.lock` by
[`tools/generate_dependency_provenance.py`](tools/generate_dependency_provenance.py).
Do not edit it by hand; CI checks that it is current.

- Package: `whykey 1.0.1`
- Rust edition: `2024`
- Declared MSRV: `1.85`
- Dependency source: crates.io registry entries pinned by `Cargo.lock`
- Checksums: Cargo registry checksums recorded in `Cargo.lock`
- Verified toolchains: stable and Rust `1.85.0`
- Verified host in this repository: Linux `x86_64-unknown-linux-gnu`
- Other release targets, package builders, and distro toolchain versions
  remain outside this verification record.

| Package | Version | Source | SHA-256 registry checksum | Direct dependency |
| --- | --- | --- | --- | --- |
| `itoa` | `1.0.18` | `registry+https://github.com/rust-lang/crates.io-index` | `8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682` | no |
| `libc` | `0.2.189` | `registry+https://github.com/rust-lang/crates.io-index` | `3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2` | yes |
| `memchr` | `2.8.3` | `registry+https://github.com/rust-lang/crates.io-index` | `cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98` | no |
| `proc-macro2` | `1.0.107` | `registry+https://github.com/rust-lang/crates.io-index` | `985e7ec9bb745e6ce6535b544d84d6cd6f7ad8bd711c398938ae983b91a766d9` | no |
| `quote` | `1.0.47` | `registry+https://github.com/rust-lang/crates.io-index` | `1fbf4db142a473a8d80c26bbf18454ed458bf8d26c8219c331daecfdbd079001` | no |
| `serde` | `1.0.229` | `registry+https://github.com/rust-lang/crates.io-index` | `4148590afebada386688f18773da617792bf2ef03ffc1e4cbd2b1d45b023e0ba` | yes |
| `serde_core` | `1.0.229` | `registry+https://github.com/rust-lang/crates.io-index` | `67dca2c9c51e58a4791a4b1ed58308b39c64224d349a935ab5039aa360942a48` | no |
| `serde_derive` | `1.0.229` | `registry+https://github.com/rust-lang/crates.io-index` | `e7a5d71263a5a7d47b41f6b3f06ba276f10cc18b0931f1799f710578e2309348` | no |
| `serde_json` | `1.0.151` | `registry+https://github.com/rust-lang/crates.io-index` | `c841b55ecdae098c80dcae9cf767f6f8a0c2cdb3416bbef72181df4d0fe73f14` | yes |
| `syn` | `3.0.5` | `registry+https://github.com/rust-lang/crates.io-index` | `12df2e0110f65b775f769bb17ef989067a1d931b2eb822bd4346631eeada89f9` | no |
| `unicode-ident` | `1.0.24` | `registry+https://github.com/rust-lang/crates.io-index` | `e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75` | no |
| `zmij` | `1.0.23` | `registry+https://github.com/rust-lang/crates.io-index` | `29666d0abbfad1e3dc4dcf6144730dd3a3ab225bbbdac83319345b1b44ccfc1b` | no |

The lockfile remains the authoritative input for reproducible Cargo
builds. A checksum proves the selected registry package contents; it
does not claim that every downstream distro rebuild or native package
has been executed. Release archives carry a separate SHA-256 checksum
for the assembled archive.
