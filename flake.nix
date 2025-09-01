{
  description = "nanduti - Multi-backend NWC (Nostr Wallet Connect) implementation";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
          targets = [ "x86_64-unknown-linux-musl" ];
        };
      in
      {
        # Default package: static musl build
        packages.default = let
          rustPlatformMusl = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
        in rustPlatformMusl.buildRustPackage {
          pname = "nanduti";
          version = "0.1.0";
          src = ./.;
          
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          
          nativeBuildInputs = with pkgs; [
            pkg-config
            rustToolchain
            pkgsStatic.stdenv.cc
          ];
          
          buildInputs = with pkgs.pkgsStatic; [
            # Add any static libraries if needed
          ];
          
          # Force cargo to use the musl target
          CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${pkgs.pkgsStatic.stdenv.cc}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}cc";
          CC_x86_64_unknown_linux_musl = "${pkgs.pkgsStatic.stdenv.cc}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}cc";
          CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static -C link-arg=-static";
          
          # Override buildPhase to use the correct target
          buildPhase = ''
            runHook preBuild
            
            echo "Building with musl target for static binary..."
            cargo build \
              --release \
              --target x86_64-unknown-linux-musl \
              --offline \
              -j $NIX_BUILD_CORES
            
            runHook postBuild
          '';
          
          installPhase = ''
            runHook preInstall
            
            mkdir -p $out/bin
            cp target/x86_64-unknown-linux-musl/release/nanduti $out/bin/
            
            runHook postInstall
          '';
          
          # Ensure static linking
          doCheck = false; # Tests don't work well with static linking
          
          # Verify the binary is statically linked
          postInstall = ''
            echo "Checking if binary is statically linked..."
            file $out/bin/nanduti
            # Strip the binary to reduce size
            ${pkgs.binutils}/bin/strip $out/bin/nanduti
          '';
          
          meta = with pkgs.lib; {
            description = "Multi-backend NWC (Nostr Wallet Connect) implementation support";
            homepage = "https://github.com/user/nanduti";
            license = licenses.mit;
            maintainers = [ ];
          };
        };
        
        # Alternative dynamic build (non-static)
        packages.nanduti-dynamic = pkgs.rustPlatform.buildRustPackage {
          pname = "nanduti-dynamic";
          version = "0.1.0";
          src = ./.;
          
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          
          nativeBuildInputs = with pkgs; [
            pkg-config
            rustToolchain
          ];
          
          buildInputs = with pkgs; [
            # Add any dynamic libraries if needed
          ];
          
          meta = with pkgs.lib; {
            description = "Multi-federation Fedimint wallet with NWC support (dynamic build)";
            homepage = "https://github.com/user/nanduti";
            license = licenses.mit;
            maintainers = [ ];
          };
        };
        
        # Docker image output
        packages.docker = pkgs.dockerTools.buildLayeredImage {
          name = "nanduti";
          tag = "latest";
          
          contents = with pkgs; [
            # Include CA certificates for HTTPS
            cacert
            # Include basic utilities
            coreutils
            bash
          ];
          
          config = {
            Entrypoint = [ "${self.packages.${system}.default}/bin/nanduti" ];
            Cmd = [ "serve" ];  # Default command
            Env = [
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              "RUST_LOG=info"
            ];
            WorkingDir = "/data";
            Volumes = {
              "/data" = {};
            };
            ExposedPorts = {
              "3517/tcp" = {};
            };
            Labels = {
              "org.opencontainers.image.description" = "Multi-backend NWC (Nostr Wallet Connect) implementation";
              "org.opencontainers.image.source" = "https://github.com/user/nanduti";
              "org.opencontainers.image.licenses" = "MIT";
            };
          };
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            bashInteractive
            # Use regular rust toolchain for development (not musl)
            (rust-bin.stable.latest.default.override {
              extensions = [ "rust-src" "rust-analyzer" ];
            })
            pkg-config
            gh
            cargo-edit
            cargo-outdated
            # Build dependencies
            clang
            libclang.lib
          ];

          # Set libclang path for proc-macro compilation
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          
          shellHook = ''
            # Automatically configure Git hooks for code quality
            if [ -d .git ] && [ -d .githooks ]; then
              current_hooks_path=$(git config core.hooksPath || echo "")
              if [ "$current_hooks_path" != ".githooks" ]; then
                echo "📎 Setting up Git hooks for code quality checks..."
                git config core.hooksPath .githooks
                echo "✅ Git hooks configured automatically!"
                echo "   • pre-commit: Checks code formatting"
                echo "   • pre-push: Runs formatting and clippy checks"
                echo ""
                echo "To disable: git config --unset core.hooksPath"
              fi
            fi
          '';
        };
      }
    );
}