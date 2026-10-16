# Dependency provenance and tested versions

This file is generated from `Cargo.toml`, `Cargo.lock`, and
`src/xkb_keysyms.txt` by
[`tools/generate_dependency_provenance.py`](tools/generate_dependency_provenance.py).
Do not edit it by hand; CI checks that it is current.

- Package: `whykey 2.4.1`
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
| `arraydeque` | `0.5.1` | `registry+https://github.com/rust-lang/crates.io-index` | `7d902e3d592a523def97af8f317b08ce16b7ab854c1985a0c671e6f15cebc236` | no |
| `bitflags` | `2.13.2` | `registry+https://github.com/rust-lang/crates.io-index` | `3ded4057c258ba199e2d26386d3af3780957ecaee6c4ef4041c6b4b8b97c0b06` | no |
| `cc` | `1.7.0` | `registry+https://github.com/rust-lang/crates.io-index` | `50a649af8a827553c29fb0cb4bd4a6f1a0dd695bd3232b9bc98bd9c8a3ffbb8b` | no |
| `downcast-rs` | `1.2.1` | `registry+https://github.com/rust-lang/crates.io-index` | `75b325c5dbd37f80359721ad39aca5a29fb04c89279657cffdda8736d0c0b9d2` | no |
| `errno` | `0.3.14` | `registry+https://github.com/rust-lang/crates.io-index` | `39cab71617ae0d63f51a36d69f866391735b51691dbda63cf6f96d042b63efeb` | no |
| `find-msvc-tools` | `0.1.14` | `registry+https://github.com/rust-lang/crates.io-index` | `aedcfb3409746eddb02b9e19ebda1c3394f759a152e48ee875a0844d1b955484` | no |
| `foldhash` | `0.2.0` | `registry+https://github.com/rust-lang/crates.io-index` | `77ce24cb58228fbb8aa041425bb1050850ac19177686ea6e0f41a70416f56fdb` | no |
| `gethostname` | `1.1.0` | `registry+https://github.com/rust-lang/crates.io-index` | `1bd49230192a3797a9a4d6abe9b3eed6f7fa4c8a8a4947977c6f80025f92cbd8` | no |
| `hashbrown` | `0.17.1` | `registry+https://github.com/rust-lang/crates.io-index` | `ed5909b6e89a2db4456e54cd5f673791d7eca6732202bbf2a9cc504fe2f9b84a` | no |
| `hashlink` | `0.12.2` | `registry+https://github.com/rust-lang/crates.io-index` | `a596f1b20ed2cc5ecac41a164aaebc7258057060f06c0cf7a2ba3991ee7990fb` | no |
| `itoa` | `1.0.18` | `registry+https://github.com/rust-lang/crates.io-index` | `8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682` | no |
| `libc` | `0.2.189` | `registry+https://github.com/rust-lang/crates.io-index` | `3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2` | yes |
| `linux-raw-sys` | `0.12.1` | `registry+https://github.com/rust-lang/crates.io-index` | `32a66949e030da00e8c7d4434b251670a91556f4144941d37452769c25d58a53` | no |
| `memchr` | `2.8.3` | `registry+https://github.com/rust-lang/crates.io-index` | `cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98` | no |
| `pkg-config` | `0.3.34` | `registry+https://github.com/rust-lang/crates.io-index` | `f6b464fbc74e149a392436b17d523f769e057cb6877f6a5c4618bc6f11800548` | no |
| `proc-macro2` | `1.0.107` | `registry+https://github.com/rust-lang/crates.io-index` | `985e7ec9bb745e6ce6535b544d84d6cd6f7ad8bd711c398938ae983b91a766d9` | no |
| `quick-xml` | `0.41.0` | `registry+https://github.com/rust-lang/crates.io-index` | `e660451e55124f798a69a5af3f49ccfbefbd41910eefd25caf2393e1f3473ec1` | no |
| `quote` | `1.0.47` | `registry+https://github.com/rust-lang/crates.io-index` | `1fbf4db142a473a8d80c26bbf18454ed458bf8d26c8219c331daecfdbd079001` | no |
| `rustix` | `1.1.5` | `registry+https://github.com/rust-lang/crates.io-index` | `891efababe418670775f199f0d233d84843c227a0949a883ce15b37c78d6629d` | no |
| `serde` | `1.0.229` | `registry+https://github.com/rust-lang/crates.io-index` | `4148590afebada386688f18773da617792bf2ef03ffc1e4cbd2b1d45b023e0ba` | yes |
| `serde_core` | `1.0.229` | `registry+https://github.com/rust-lang/crates.io-index` | `67dca2c9c51e58a4791a4b1ed58308b39c64224d349a935ab5039aa360942a48` | no |
| `serde_derive` | `1.0.229` | `registry+https://github.com/rust-lang/crates.io-index` | `e7a5d71263a5a7d47b41f6b3f06ba276f10cc18b0931f1799f710578e2309348` | no |
| `serde_json` | `1.0.151` | `registry+https://github.com/rust-lang/crates.io-index` | `c841b55ecdae098c80dcae9cf767f6f8a0c2cdb3416bbef72181df4d0fe73f14` | yes |
| `shlex` | `2.0.1` | `registry+https://github.com/rust-lang/crates.io-index` | `f8fadd59c855ef2080decdef8ff161eb6661b86933c9d82e5ba29dc602a55aba` | no |
| `smallvec` | `1.16.3` | `registry+https://github.com/rust-lang/crates.io-index` | `5b3dc8af474f516a851ff4bd12db780f948b9250ad37211e4eec0bccea54e01b` | no |
| `syn` | `3.0.5` | `registry+https://github.com/rust-lang/crates.io-index` | `12df2e0110f65b775f769bb17ef989067a1d931b2eb822bd4346631eeada89f9` | no |
| `unicode-ident` | `1.0.24` | `registry+https://github.com/rust-lang/crates.io-index` | `e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75` | no |
| `wayland-backend` | `0.3.17` | `registry+https://github.com/rust-lang/crates.io-index` | `38a91b4eaddff87b1cd1074985e3713da4af2c49742d1b356b2c01670a67a078` | no |
| `wayland-client` | `0.31.15` | `registry+https://github.com/rust-lang/crates.io-index` | `e3c36a0f861ad76d0901f2800b46321410d9f73f2ea88aac0650d86c32688073` | yes |
| `wayland-protocols` | `0.32.12` | `registry+https://github.com/rust-lang/crates.io-index` | `563a85523cade2429938e790815fd7319062103b9f4a2dc806e9b53b95982d8f` | yes |
| `wayland-scanner` | `0.31.11` | `registry+https://github.com/rust-lang/crates.io-index` | `338e30461b3a2b67d70eb30a6d89f8e0c93a833e07d2ae89085cd070c4a00ac0` | no |
| `wayland-server` | `0.31.14` | `registry+https://github.com/rust-lang/crates.io-index` | `0dde9c29be0f723a573977de51ee455bf3dfa03652730a74f9dd3b337e374d75` | no |
| `wayland-sys` | `0.31.11` | `registry+https://github.com/rust-lang/crates.io-index` | `d8eab23fefc9e41f8e841df4a9c707e8a8c4ed26e944ef69297184de2785e3be` | no |
| `windows-link` | `0.2.1` | `registry+https://github.com/rust-lang/crates.io-index` | `f0805222e57f7521d6a62e36fa9163bc891acd422f971defe97d64e70d0a4fe5` | no |
| `windows-sys` | `0.61.2` | `registry+https://github.com/rust-lang/crates.io-index` | `ae137229bcbd6cdf0f7b80a31df61766145077ddf49416a728b02cb3921ff3fc` | no |
| `x11rb` | `0.14.0` | `registry+https://github.com/rust-lang/crates.io-index` | `5a8885a854a8bfdf87a301e53e41b17c5f8f33639903131338b997b1eb614f44` | yes |
| `x11rb-protocol` | `0.14.0` | `registry+https://github.com/rust-lang/crates.io-index` | `acf4d1bc32aa46eec18caa634ec3cf4c05bfa151f12b93b510b15190f69a1ca8` | no |
| `yaml-rust2` | `0.13.0` | `registry+https://github.com/rust-lang/crates.io-index` | `57e5b818a27a4cd30884ea380857a5e56f7ec3ba24a3990a3cc0b95af3238e18` | yes |
| `zmij` | `1.0.23` | `registry+https://github.com/rust-lang/crates.io-index` | `29666d0abbfad1e3dc4dcf6144730dd3a3ab225bbbdac83319345b1b44ccfc1b` | no |

The lockfile remains the authoritative input for reproducible Cargo
builds. A checksum proves the selected registry package contents; it
does not claim that every downstream distro rebuild or native package
has been executed. Release archives carry a separate SHA-256 checksum
for the assembled archive.

## Vendored data

`src/xkb_keysyms.txt` is compiled into the binary with `include_str!` and
maps X11 keysym names to Unicode code points for XKB text prediction.

- Source: `keysymdef.h` from X.Org xorgproto 2025.1
  (`include/X11/keysymdef.h`)
- Derivation: every `#define XK_<name> 0x… /* U+XXXX` or `// U+XXXX`
  line, in file order; the `<U+…>` and `(U+…)` annotations, which
  keysymdef.h uses for duplicate or inexact correspondences, are excluded
- Entries: 1632
- SHA-256: `24d80dd04f6a478f8fbebe0ceea6d4e3eebbfe6aa17fa822751e0369e0d09aa9`
- License: the X11-style permission notices of `keysymdef.h`, reproduced
  below

```text
Copyright 1987, 1994, 1998  The Open Group

Permission to use, copy, modify, distribute, and sell this software and its
documentation for any purpose is hereby granted without fee, provided that
the above copyright notice appear in all copies and that both that
copyright notice and this permission notice appear in supporting
documentation.

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE OPEN GROUP BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

Except as contained in this notice, the name of The Open Group shall
not be used in advertising or otherwise to promote the sale, use or
other dealings in this Software without prior written authorization
from The Open Group.


Copyright 1987 by Digital Equipment Corporation, Maynard, Massachusetts

                        All Rights Reserved

Permission to use, copy, modify, and distribute this software and its
documentation for any purpose and without fee is hereby granted,
provided that the above copyright notice appear in all copies and that
both that copyright notice and this permission notice appear in
supporting documentation, and that the name of Digital not be
used in advertising or publicity pertaining to distribution of the
software without specific, written prior permission.

DIGITAL DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE, INCLUDING
ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS, IN NO EVENT SHALL
DIGITAL BE LIABLE FOR ANY SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES OR
ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS
SOFTWARE.
```
