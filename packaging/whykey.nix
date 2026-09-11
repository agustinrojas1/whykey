{ lib, rustPlatform, util-linux }:

rustPlatform.buildRustPackage {
  pname = "whykey";
  version = "1.0.2";

  src = lib.cleanSource ../.;
  cargoLock.lockFile = ../Cargo.lock;

  nativeCheckInputs = [ util-linux ];

  postInstall = ''
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
