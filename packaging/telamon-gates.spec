# Telamon Gates for Telamon OS.

# No debuginfo subpackage: the Rust flags below keep symbols and the binary is
# shipped as built.
%global debug_package %{nil}

Name:           telamon-gates
Version:        1.3.0
Release:        1%{?dist}
Summary:        Telamon Gates, the AI chat of Telamon OS
License:        MIT
URL:            https://github.com/EternalCoder454/telamon-gates
Source0:        telamon-gates-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
# %%build_rustflags
BuildRequires:  rust-srpm-macros
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  cmake
BuildRequires:  ninja-build
BuildRequires:  corrosion
# Cargo fetches the telamon-framework crates from GitHub.
BuildRequires:  git-core
BuildRequires:  desktop-file-utils
# The agent's commands run in a bubblewrap sandbox (the tests run one too).
BuildRequires:  bubblewrap
# Downloads from Hugging Face use the system's TLS.
BuildRequires:  pkgconfig(openssl)
BuildRequires:  libappstream-glib
BuildRequires:  cmake(Qt6Core)
BuildRequires:  cmake(Qt6Gui)
BuildRequires:  cmake(Qt6Qml)
BuildRequires:  cmake(Qt6Quick)
BuildRequires:  cmake(Qt6QuickControls2)
BuildRequires:  cmake(Qt6Widgets)
BuildRequires:  cmake(Qt6QmlTools)
BuildRequires:  qt6-qtbase-devel
BuildRequires:  cmake(KF6DBusAddons)
BuildRequires:  cmake(KF6WindowSystem)
# QML modules qmlcachegen resolves at build time (not linked). telamon-ui comes
# from the Telamon framework, which is in no repository: install its RPMs
# first (scripts/dev.sh does, given ATLAS_LOCAL_RPMS).
BuildRequires:  kf6-kirigami-devel
BuildRequires:  telamon-ui >= 2.0.6

Requires:       kf6-kirigami
Requires:       telamon-ui >= 2.0.6
Requires:       kf6-qqc2-desktop-style
Requires:       qt6-qtdeclarative
Requires:       qt6-qtsvg
# The model server (llama.cpp with Vulkan); without it the demo answers.
Requires:       bubblewrap
# Keeps the web search key in the system keyring (/usr/bin/secret-tool).
Requires:       libsecret
Recommends:     telamon-llama

%description
Telamon Gates is the AI chat of Telamon OS: conversations with a local model,
kept as plain files on your computer.

%prep
%autosetup -n telamon-gates-%{version}

%build
# NETWORK: cargo (Corrosion runs it with --locked) fetches crates.io and the
# pinned telamon-framework crates during %%build.
export CARGO_HOME=${CARGO_HOME:-%{_builddir}/cargo-home}
# opt-level "s" last, so it wins over the macro's 3 (and matches Cargo.toml's
# release profile, which the flags would otherwise override).
export RUSTFLAGS="%{build_rustflags} -Copt-level=s --remap-path-prefix=$PWD=. --remap-path-prefix=$CARGO_HOME=cargo"
export CARGO_PROFILE_RELEASE_STRIP=none
%global _vpath_srcdir apps/telamon-gates
%cmake -G Ninja -DCMAKE_BUILD_TYPE=Release
%cmake_build

%install
%cmake_install

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/net.eterneon.telamon.gates.desktop
appstream-util validate-relax --nonet \
    %{buildroot}%{_datadir}/metainfo/net.eterneon.telamon.gates.metainfo.xml

%files
%license LICENSE
%{_bindir}/telamon-gates
%{_datadir}/applications/net.eterneon.telamon.gates.desktop
%{_datadir}/icons/hicolor/scalable/apps/net.eterneon.telamon.gates.svg
%{_datadir}/metainfo/net.eterneon.telamon.gates.metainfo.xml

%changelog
* Fri Oct 09 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 1.3.0-1
- Starts faster (152 to 113 ms), uses less memory (123 to 110 MB idle) and is smaller (16.4 to 6.9 MB)
- Graphics memory limit: models stop before the card fills (95% by default)
- Settings: System Prompt first, foldable sections
- Models: browse open models by the UGI Leaderboard and find their GGUF
- The web search key is kept through secret-tool (libsecret)

* Fri Oct 09 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 1.2.0-1
- Web search (Brave, Tavily or SearXNG) for Chat, Code and Agent, with the key in the keyring
- Deep Research mode: plans, searches, reads and writes a cited report
- Model server: retries a load that runs out of memory, stops crash loops, refuses unsupported models
- Downloads check free space and keep partial downloads to resume
- Conversations: format version, damaged files set aside, one backup each
- One window, a first-run check for the model server, and a log file

* Fri Oct 09 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 1.1.0-1
- Recommended models for coding, chat and stories, and small cards
- Model for Code: Code and Agent mode can use their own model
- Brief reasoning in Chat and Story

* Fri Oct 09 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 1.0.1-1
- Faster start: the agent's sandbox is checked when Agent mode is first used

* Fri Oct 09 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 1.0.0-1
- Local models with llama.cpp, SystemOne and modes, Agent mode in a sandbox,
  the Fleet, attachments, edit/branch/export, and performance work

* Thu Oct 08 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.1.0-1
- First package
