{
  description = "weft-sim: development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          inherit (pkgs) lib stdenv;
          arch = stdenv.hostPlatform.parsed.cpu.name;
          # Prototype guest kernel: the prebuilt nixpkgs kernel. To be replaced by weft's own
          # minimal configuration (see weft-design docs/04-decisions.md).
          kernel = pkgs.linuxPackages_latest.kernel;
          rust = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
            targets = lib.optional stdenv.hostPlatform.isLinux "${arch}-unknown-linux-musl";
          };
        in
        {
          default = pkgs.mkShell {
            packages = [
              rust
              pkgs.just
              pkgs.cargo-deny
              pkgs.taplo
              pkgs.typos
              pkgs.reuse
              pkgs.strictdoc
              pkgs.dtc
            ]
            ++ lib.optionals stdenv.hostPlatform.isLinux [
              # Guest image and kernel module.
              pkgs.qemu_kvm
              pkgs.cpio
              pkgs.xz
              pkgs.gzip
              pkgs.gnumake
              pkgs.gcc
              pkgs.elfutils
              pkgs.bc
              pkgs.pahole
            ];
            env = lib.optionalAttrs stdenv.hostPlatform.isLinux {
              WEFT_GUEST_ARCH = arch;
              WEFT_KERNEL_IMAGE = "${kernel}/${if stdenv.hostPlatform.isx86 then "bzImage" else "Image"}";
              WEFT_KERNEL_BUILD = "${kernel.dev}/lib/modules/${kernel.modDirVersion}/build";
              WEFT_KERNEL_MODULES = "${kernel.modules}/lib/modules/${kernel.modDirVersion}";
              WEFT_BUSYBOX = "${pkgs.pkgsStatic.busybox}/bin/busybox";
            };
          };
        }
      );
    };
}
