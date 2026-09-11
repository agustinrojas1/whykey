Name:           whykey
Version:        1.0.2
Release:        1%{?dist}
Summary:        Explain where a Linux key combination is handled
License:        MIT AND (MIT OR Apache-2.0) AND (MIT OR Unlicense) AND Unicode-3.0
URL:            https://github.com/agustinrojas1/whykey
Source0:        %{url}/releases/download/v%{version}/%{name}-%{version}-source.tar.gz
BuildRequires:  cargo
BuildRequires:  rust-packaging
BuildRequires:  util-linux-script

%description
Whykey diagnoses the path of a keyboard event through Linux desktops,
terminals, TTYs, multiplexers, shells, and supported applications. It is
read-only and never executes the shortcut under investigation.

%prep
%autosetup -n %{name}-%{version}
%cargo_prep -v vendor

%build
%cargo_build
%cargo_vendor_manifest

%check
%cargo_test

%install
install -Dpm0755 target/release/whykey %{buildroot}%{_bindir}/whykey
install -Dpm0644 whykey.1 %{buildroot}%{_mandir}/man1/whykey.1
install -Dpm0644 LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE
install -Dpm0644 README.md %{buildroot}%{_docdir}/%{name}/README.md
install -Dpm0644 SPEC.md %{buildroot}%{_docdir}/%{name}/SPEC.md
install -Dpm0644 CHANGELOG.md %{buildroot}%{_docdir}/%{name}/CHANGELOG.md
install -Dpm0644 SUPPORT_MATRIX.md %{buildroot}%{_docdir}/%{name}/SUPPORT_MATRIX.md
install -Dpm0644 ADAPTER_INVENTORY.md %{buildroot}%{_docdir}/%{name}/ADAPTER_INVENTORY.md
install -Dpm0644 DEPENDENCY_PROVENANCE.md %{buildroot}%{_docdir}/%{name}/DEPENDENCY_PROVENANCE.md
install -Dpm0644 DEFECT_REGISTER.md %{buildroot}%{_docdir}/%{name}/DEFECT_REGISTER.md
install -Dpm0644 support-matrix.json %{buildroot}%{_docdir}/%{name}/support-matrix.json

%files
%{_bindir}/whykey
%{_mandir}/man1/whykey.1*
%license %{_licensedir}/%{name}/LICENSE
%license cargo-vendor.txt
%doc %{_docdir}/%{name}/README.md
%doc %{_docdir}/%{name}/SPEC.md
%doc %{_docdir}/%{name}/CHANGELOG.md
%doc %{_docdir}/%{name}/SUPPORT_MATRIX.md
%doc %{_docdir}/%{name}/ADAPTER_INVENTORY.md
%doc %{_docdir}/%{name}/DEPENDENCY_PROVENANCE.md
%doc %{_docdir}/%{name}/DEFECT_REGISTER.md
%doc %{_docdir}/%{name}/support-matrix.json

%changelog
* Fri Sep 11 2026 Agustín Rojas <agustinrojas1@users.noreply.github.com> - 1.0.2-1
- Simplify inspection and reporting contracts and add differential validation

* Wed Sep 09 2026 Agustín Rojas <agustinrojas1@users.noreply.github.com> - 1.0.1-1
- Harden temporary Hyprland capture cleanup and offline diagnostics

* Mon Sep 07 2026 Agustín Rojas <agustinrojas1@users.noreply.github.com> - 1.0.0-1
- Whykey 1.0.0 release
