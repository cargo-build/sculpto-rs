# Build shell for the interactive viewer only.
#
# The headless viewer needs no windowing libraries; this shell exists so the
# optional `--interactive` mode can compile against Wayland/X11. The Rust
# toolchain stays the operator's rustup toolchain.
{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
  ];
  buildInputs = with pkgs; [
    wayland
    libxkbcommon
    libx11
    libxcursor
    libxi
    libxrandr
  ];
}
