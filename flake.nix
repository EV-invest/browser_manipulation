{
  nixConfig = {
    extra-substituters = [ "https://valeratrades.cachix.org" ];
    extra-trusted-public-keys = [ "valeratrades.cachix.org-1:gXVwhzO5YB+BaiEJYT48qZgzdaErGQew6xtZcz4Fo1Q=" ];
  };

  inputs = {
    v_flakes.url = "github:valeratrades/v_flakes?ref=v1.6";
  };

  outputs = { self, v_flakes }:
    let
      inherit (v_flakes) flake-utils pre-commit-hooks;
      manifest = (builtins.fromTOML (builtins.readFile ./browser_manipulation/Cargo.toml)).package;
      pname = manifest.name;
    in
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import v_flakes.default_nixpkgs { inherit system; };
        rust = v_flakes.rs.default_nightly system;
        pre-commit-check = pre-commit-hooks.lib.${system}.run (v_flakes.files.preCommit { inherit pkgs; });
        stdenv = pkgs.stdenvAdapters.useMoldLinker pkgs.stdenv;

        rs = v_flakes.rs {
          inherit pkgs rust;
          build = {
            deny = false;
            workspace = let deprecate_by = "v1.0.0"; in {
              "./browser_manipulation/" = [{ deprecate = { by_version = deprecate_by; force = true; }; }];
            };
          };
        };
        patchright = pkgs.stdenvNoCC.mkDerivation {
          pname = "patchright-core";
          version = "1.63.0";
          src = pkgs.fetchurl {
            url = "https://registry.npmjs.org/patchright-core/-/patchright-core-1.63.0.tgz";
            sha256 = "0v5fqh1qhbfmghhdcckm8hiv039bnh0cchr1s57zclbyfz2b6649";
          };
          installPhase = ''
            mkdir -p $out/package
            cp -r . $out/package/
          '';
        };
        github = v_flakes.github {
          inherit pkgs pname rs;
          enable = true;
          lastSupportedVersion = "nightly-2026-09-28";
          jobs.default = true;
        };
        readme = v_flakes.readme-fw {
          inherit pkgs pname;
          defaults = true;
          lastSupportedVersion = "nightly-1.100";
          rootDir = ./.;
          badges = [ "msrv" "crates_io" "docs_rs" "loc" "ci" ];
        };
        combined = v_flakes.utils.combine { inherit rust; modules = [ rs github readme ]; };
      in
      {
        packages =
          let
            build_rust = v_flakes.rs.build_nightly system;
            rustc = build_rust;
            cargo = build_rust;
            rustPlatform = pkgs.makeRustPlatform {
              inherit rustc cargo stdenv;
            };
          in
          {
            default = rustPlatform.buildRustPackage {
              inherit pname;
              version = manifest.version;

              buildInputs = with pkgs; [
                openssl.dev
              ];
              nativeBuildInputs = with pkgs; [ pkg-config ];

              cargoLock.lockFile = ./Cargo.lock;
              src = pkgs.lib.cleanSource ./.;
            };
            inherit patchright;
          };

        devShells.default =
          with pkgs;
          mkShell {
            inherit stdenv;
            shellHook =
              pre-commit-check.shellHook
              + combined.shellHook
              + ''
                cp -f ${(v_flakes.files.treefmt) { inherit pkgs; }} ./.treefmt.toml
              '';

            packages = [
              mold
              openssl
              pkg-config
              rust
              nodejs
              chromium
            ] ++ pre-commit-check.enabledPackages ++ combined.enabledPackages;

            env.RUST_BACKTRACE = 1;
            env.RUST_LIB_BACKTRACE = 0;
            env.PLAYWRIGHT_CLI_JS = "${patchright}/package/cli.js";
            env.PLAYWRIGHT_NODE_EXE = "${pkgs.nodejs}/bin/node";
            env.PLAYWRIGHT_SKIP_DRIVER_DOWNLOAD = "1";
            env.BM_TEST_CHROME = "${pkgs.chromium}/bin/chromium";
          };
      }
    );
}
