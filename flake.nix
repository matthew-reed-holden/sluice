{
  description = "Development environment with Python (via uv) and Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Select stable Rust toolchain with common components
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
            "rustfmt"
          ];
        };

        # Common packages for both platforms
        commonPackages = with pkgs; [
          # Python via uv
          uv
          python312 # Provide a Nix-built Python for uv to use

          # Rust toolchain
          rustToolchain
          cargo-watch
          cargo-edit
          cargo-audit

          # Common development tools
          pkg-config
          openssl
          nixfmt-rfc-style # Nix formatter
        ];

        # Linux-specific packages
        linuxPackages = with pkgs; [
          # Additional libraries that may be needed by Python packages from PyPI
          stdenv.cc.cc.lib
          zlib
          libffi
        ];

        # Darwin-specific packages
        # Note: Frameworks like Security, SystemConfiguration, etc. are now included
        # by default in the SDK and don't need to be explicitly listed
        darwinPackages = with pkgs; [
          libiconv
          darwin.cctools
        ];

        # Platform-specific packages
        platformPackages = if pkgs.stdenv.isDarwin then darwinPackages else linuxPackages;

        # Build LD_LIBRARY_PATH for Linux to handle dynamically-linked Python packages
        # This is needed for packages like numpy that vendor dynamically-linked libraries
        linuxLibraryPath = pkgs.lib.makeLibraryPath (
          with pkgs;
          [
            stdenv.cc.cc.lib
            zlib
            libffi
            openssl
          ]
        );

      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = commonPackages ++ platformPackages;

          # Environment variables
          env = {
            # Configure uv to use the Nix-provided Python instead of downloading its own
            # This is critical for NixOS/Nix compatibility
            UV_PYTHON = "${pkgs.python312}/bin/python3";

            # Prevent uv from downloading Python binaries (they won't work on NixOS)
            UV_PYTHON_DOWNLOADS = "never";
          };

          shellHook = ''
            echo "🦀 Rust + 🐍 Python (uv) Development Environment"
            echo ""
            echo "Rust version: $(rustc --version)"
            echo "Cargo version: $(cargo --version)"
            echo "uv version: $(uv --version)"
            echo "Python (for uv): ${pkgs.python312}/bin/python3"
            echo ""

            # Set up LD_LIBRARY_PATH on Linux for Python packages with native dependencies
            ${pkgs.lib.optionalString pkgs.stdenv.isLinux ''
              export LD_LIBRARY_PATH="${linuxLibraryPath}:$LD_LIBRARY_PATH"
              echo "LD_LIBRARY_PATH configured for native Python package dependencies"
            ''}

            # Helpful hints
            echo "Quick start:"
            echo "  - Create a Python project: uv init"
            echo "  - Add a Python dependency: uv add <package>"
            echo "  - Run Python script: uv run python script.py"
            echo "  - Create a Rust project: cargo init"
            echo "  - Build Rust project: cargo build"
            echo ""
          '';
        };

        # Alternative shell with nightly Rust
        devShells.nightly = pkgs.mkShell {
          buildInputs = [
            # Use nightly Rust instead
            (pkgs.rust-bin.nightly.latest.default.override {
              extensions = [
                "rust-src"
                "rust-analyzer"
                "clippy"
                "rustfmt"
              ];
            })
            pkgs.cargo-watch
            pkgs.cargo-edit
            pkgs.cargo-audit

            # Python via uv
            pkgs.uv
            pkgs.python312

            # Common tools
            pkgs.pkg-config
            pkgs.openssl
          ]
          ++ platformPackages;

          env = {
            UV_PYTHON = "${pkgs.python312}/bin/python3";
            UV_PYTHON_DOWNLOADS = "never";
          };

          shellHook = ''
            echo "🦀 Rust (nightly) + 🐍 Python (uv) Development Environment"
            echo ""
            echo "Rust version: $(rustc --version)"
            echo "Cargo version: $(cargo --version)"
            echo "uv version: $(uv --version)"
            echo ""

            ${pkgs.lib.optionalString pkgs.stdenv.isLinux ''
              export LD_LIBRARY_PATH="${linuxLibraryPath}:$LD_LIBRARY_PATH"
            ''}
          '';
        };

        # Provide the packages for use in other flakes
        packages = {
          inherit rustToolchain;
        };
      }
    );
}
