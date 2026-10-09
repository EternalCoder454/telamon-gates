# llama.cpp's llama-server, built with Vulkan, for Telamon Gates.
#
# Only the server, statically linked to llama.cpp's own libraries, installed
# where nothing else looks (/usr/libexec/telamon-llama): it cannot clash with
# Fedora's llama-cpp, and Gates starts it itself. Model downloading (OpenSSL)
# and the embedded web UI are left out: Gates downloads models, and the
# server is for Gates alone.

%global debug_package %{nil}
# The source tarball's sha256, checked in %%prep.
%global source_sha256 09b36dba235fcac180efff18514da7038e65d8e5b9e4aefa59238468abfdec12

Name:           telamon-llama
Version:        0.6.0
Release:        1%{?dist}
Summary:        llama.cpp's server with Vulkan, for Telamon Gates
License:        MIT
URL:            https://github.com/ggml-org/llama.cpp
Source0:        https://github.com/ggml-org/llama.cpp/archive/refs/tags/v%{version}/llama.cpp-%{version}.tar.gz

BuildRequires:  cmake
BuildRequires:  ninja-build
BuildRequires:  gcc-c++
BuildRequires:  libgomp
BuildRequires:  vulkan-headers
BuildRequires:  vulkan-loader-devel
BuildRequires:  spirv-headers-devel
BuildRequires:  glslc

Requires:       vulkan-loader
# The Vulkan drivers for AMD and Intel (RADV, ANV); without one it runs on
# the CPU.
Recommends:     mesa-vulkan-drivers
Provides:       telamon-llama-server = %{version}-%{release}

%description
llama-server from llama.cpp, built with the Vulkan backend, for Telamon
Gates to run local GGUF models on the graphics card. It is installed in
/usr/libexec/telamon-llama and is started by Gates, not by hand.

%prep
echo "%{source_sha256}  %{SOURCE0}" | sha256sum -c -
%autosetup -n llama.cpp-%{version}

%build
# Portable CPU code (the graphics card does the heavy work), no native tuning.
%cmake -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DBUILD_SHARED_LIBS=OFF \
    -DLLAMA_BUILD_IS_DEV=OFF \
    -DLLAMA_BUILD_TOOLS=ON \
    -DLLAMA_BUILD_SERVER=ON \
    -DLLAMA_BUILD_APP=OFF \
    -DLLAMA_BUILD_UI=OFF \
    -DLLAMA_BUILD_TESTS=OFF \
    -DLLAMA_BUILD_EXAMPLES=OFF \
    -DLLAMA_OPENSSL=OFF \
    -DGGML_NATIVE=OFF \
    -DGGML_VULKAN=ON \
    -DGGML_BUILD_TESTS=OFF \
    -DGGML_BUILD_EXAMPLES=OFF
%cmake_build --target llama-server

%install
install -Dpm0755 %{__cmake_builddir}/bin/llama-server \
    %{buildroot}%{_libexecdir}/telamon-llama/llama-server

%check
# It runs, and it is the version packaged.
%{buildroot}%{_libexecdir}/telamon-llama/llama-server --version 2>&1 | tee version.txt
grep -q "%{version}" version.txt

%files
%license LICENSE licenses/*
%dir %{_libexecdir}/telamon-llama
%{_libexecdir}/telamon-llama/llama-server

%changelog
* Thu Oct 08 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.6.0-1
- First package: llama.cpp v0.6.0's llama-server with Vulkan
