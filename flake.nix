{
  description = "Yocto BSP for the Nest Learning Thermostat 2nd gen (J49, TI OMAP3)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
    # Prebuilt Rust toolchains incl. rust-std for armv7-unknown-linux-musleabihf
    # (nixpkgs' pkgsCross rustc would bootstrap rustc from source, ~45 min+).
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, fenix }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        # Static musl cross toolchain for the device (ARMv7-A hard-float). Used
        # as the linker for Rust and directly for the C helpers (bptool, memdump,
        # i2cprobe, ramoops-read) in tools/kernel-firstlight.
        crossStatic = pkgs.pkgsCross.armv7l-hf-multiplatform.pkgsStatic;
        crossCC = crossStatic.stdenv.cc;
        rustTarget = "armv7-unknown-linux-musleabihf";
        rustToolchain = with fenix.packages.${system}; combine [
          stable.rustc stable.cargo stable.rust-std stable.rust-src stable.rustfmt stable.clippy
          targets.${rustTarget}.stable.rust-std
        ];
      in {
        # Build the Yocto image in the official Yocto container via kas-container
        # (sidesteps NixOS FHS assumptions). Requires a running docker daemon.
        #   kas-container build kas/nest-j49.yml
        devShells.default = pkgs.mkShell {
          name = "nest-yocto";
          packages = with pkgs; [
            kas          # provides both `kas` and `kas-container`
            git
            docker-client
            usbutils     # lsusb
            # OMAP3 USB peripheral-boot helper (tools/omap-usbboot)
            (python3.withPackages (ps: [ ps.pyusb ]))
            # Installer U-Boot DFU gadget (0451:df10): tools/dfu-install
            dfu-util
            # Device-side helpers: Rust (preferred) + static musl C cross gcc
            rustToolchain
            crossCC
          ];
          # cargo build --release --target armv7-unknown-linux-musleabihf
          # -> target/armv7-unknown-linux-musleabihf/release/<bin>, fully static.
          CARGO_BUILD_TARGET = rustTarget;
          CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER = "${crossCC.targetPrefix}cc";
          CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_RUSTFLAGS = "-C target-feature=+crt-static";
          shellHook = ''
            echo "nest-yocto dev shell (NixOS + kas-container)"
            echo "  build:  kas-container build kas/nest-j49.yml"
            echo "  shell:  kas-container shell  kas/nest-j49.yml"
            echo "  rust:   cargo build --release   (target ${rustTarget}, static)"
            echo "  dfu:    tools/dfu-install/stage{1,2,3}.sh  (dfu-util 0451:df10)"
            export KAS_CONTAINER_ENGINE=docker
          '';
        };

        # Desktop emulator for tools/j49-screen: host Rust (no device target) plus
        # the windowing libs minifb dlopen()s at runtime (NixOS has no global libs).
        #   nix develop .#emu
        #   cd tools/j49-screen && cargo build --release --features emu
        #   ./target/release/j49-screen --window --beep-emu
        devShells.emu = pkgs.mkShell {
          name = "j49-screen-emu";
          packages = with pkgs; [ cargo rustc pipewire espeak-ng ];  # pw-play for sound, espeak-ng for TTS
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
            wayland libxkbcommon libx11 libxcursor libxi libxrandr libGL
          ]);
          shellHook = ''
            echo "j49-screen emulator shell"
            echo "  cargo build --release --features emu   (in tools/j49-screen)"
            echo "  ./target/release/j49-screen --window --beep-emu"
          '';
        };
      });
}
