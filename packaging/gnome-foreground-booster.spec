Name:           gnome-foreground-booster
Version:        0.1.0
Release:        1%{?dist}
Summary:        Give the focused GNOME application a larger share of CPU time
License:        GPL-3.0-only AND (Apache-2.0 OR MIT) AND (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND (MIT OR Unlicense)
URL:            https://github.com/luminusOS/gnome-foreground-booster
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  systemd-rpm-macros
Requires:       systemd
Requires:       gnome-shell >= 41

%description
A systemd user daemon that increases the CPU weight of the currently focused
GNOME application and restores its previous weight when focus changes. It works
with Wayland and X11 without a GNOME Shell extension.

%prep
%autosetup
tar -xf %{SOURCE1}
mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
cargo build --release --locked --offline

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -d %{buildroot}%{_userunitdir}
sed 's|@BINDIR@|%{_bindir}|' data/%{name}.service.in > %{buildroot}%{_userunitdir}/%{name}.service

%check
cargo test --locked --offline

%post
%systemd_user_post %{name}.service

%preun
%systemd_user_preun %{name}.service

%files
%license LICENSE LICENSE.dependencies
%doc README.md ARCHITECTURE.md
%{_bindir}/%{name}
%{_userunitdir}/%{name}.service

%changelog
* Wed Nov 18 2026 LuminusOS <noreply@luminusos.invalid> - 0.1.0-1
- Package the foreground booster and its systemd user service.
