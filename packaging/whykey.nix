{ lib, rustPlatform, util-linux, python3 }:

rustPlatform.buildRustPackage {
  pname = "whykey";
  version = "2.3.0";

  # The checkout without build outputs, so a local `target/` or `result` is
  # never copied into the store.
  src = lib.cleanSourceWith {
    src = lib.cleanSource ../.;
    filter = path: type:
      let rel = lib.removePrefix (toString ../. + "/") (toString path);
      in !(lib.elem rel [ "target" "result" ]);
  };
  cargoLock.lockFile = ../Cargo.lock;

  # The editor extensions and their test fixtures are `#!/usr/bin/env python3`
  # scripts. The sandbox has no /usr/bin/env, and strictDeps keeps buildInputs
  # off PATH during checkPhase, so the tests need python3 as a native input and
  # patched shebangs.
  nativeCheckInputs = [ util-linux python3 ];
  postPatch = ''
    patchShebangs extensions tests/fixtures
  '';
  # Lets the fixup phase patch the installed extensions to the store python3.
  buildInputs = [ python3 ];

  postInstall = ''
    install -Dm755 extensions/whykey-nvim $out/bin/whykey-nvim
    install -Dm755 extensions/whykey-vscode $out/bin/whykey-vscode
    install -Dm644 extensions/whykey-vscode-defaults.json $out/share/whykey/whykey-vscode-defaults.json
    install -Dm755 extensions/whykey-emacs $out/bin/whykey-emacs
    install -Dm644 whykey.1 $out/share/man/man1/whykey.1
    install -Dm644 README.md $out/share/doc/whykey/README.md
    install -Dm644 SPEC.md $out/share/doc/whykey/SPEC.md
    install -Dm644 CHANGELOG.md $out/share/doc/whykey/CHANGELOG.md
    install -Dm644 SUPPORT_MATRIX.md $out/share/doc/whykey/SUPPORT_MATRIX.md
    install -Dm644 ADAPTER_INVENTORY.md $out/share/doc/whykey/ADAPTER_INVENTORY.md
    install -Dm644 DEPENDENCY_PROVENANCE.md $out/share/doc/whykey/DEPENDENCY_PROVENANCE.md
    install -Dm644 DEFECT_REGISTER.md $out/share/doc/whykey/DEFECT_REGISTER.md
    install -Dm644 support-matrix.json $out/share/doc/whykey/support-matrix.json
  '';

  meta = with lib; {
    description = "Explain where a Linux key combination is handled";
    homepage = "https://github.com/agustinrojas1/whykey";
    license = licenses.mit;
    mainProgram = "whykey";
    platforms = platforms.linux;
  };
}
