{
  description = "Sculpto Bevy viewer — build and runtime shell";

  inputs.nixpkgs.url = "flake:nixpkgs";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      # The binary links Wayland, X11 and the Vulkan loader dynamically, so the
      # dev shell exports them at runtime as well as at build time.
      runtimeLibraries = [
        pkgs.wayland
        pkgs.libxkbcommon
        pkgs.libx11
        pkgs.libxcursor
        pkgs.libxi
        pkgs.libxrandr
        pkgs.vulkan-loader
      ];
    in {
      devShells.${system}.default = pkgs.mkShell {
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs = runtimeLibraries;
        shellHook = ''
          libs="${pkgs.lib.makeLibraryPath runtimeLibraries}"
          if [ -d /run/opengl-driver/lib ]; then
            libs="/run/opengl-driver/lib:$libs"
          fi
          export LD_LIBRARY_PATH="$libs''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        '';
      };
    };
}
