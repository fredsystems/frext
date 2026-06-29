{
  description = "frext — a super lightweight text editor, with shared base + rust precommit system";

  inputs = {
    precommit.url = "github:FredSystems/pre-commit-checks";
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      precommit,
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      systems = precommit.lib.supportedSystems;
    in
    {
      ##########################################################################
      ## OVERLAY — adds `pkgs.frext` when applied
      ##########################################################################
      overlays.default = import ./nix/overlay.nix { frext-flake = self; };

      ##########################################################################
      ## HOME-MANAGER MODULE — `programs.frext` option set
      ##########################################################################
      homeManagerModules.default = import ./nix/home-manager-module.nix { frext-flake = self; };

      packages = lib.genAttrs systems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };

          rustToolchain = pkgs.rust-bin.stable.latest.default;

          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };

          runtimeLibs = [
            pkgs.libxkbcommon
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.dbus.lib
            pkgs.wayland
            pkgs.libGL
          ];

          runtimeLibPath = pkgs.lib.makeLibraryPath runtimeLibs;

          desktopItem = pkgs.makeDesktopItem {
            name = "frext";
            desktopName = "frext";
            comment = "A super lightweight text editor";
            exec = "frext %F";
            terminal = false;
            categories = [
              "Utility"
              "TextEditor"
            ];
            keywords = [
              "text"
              "editor"
              "notepad"
            ];
            startupNotify = false;
            icon = "frext";
            startupWMClass = "frext";
            mimeTypes = [ "text/plain" ];
          };

          version = "0.1.0";
        in
        {
          frext = rustPlatform.buildRustPackage {
            pname = "frext";
            inherit version;
            src = pkgs.lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.makeWrapper
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.copyDesktopItems
            ];

            buildInputs = runtimeLibs;

            desktopItems = pkgs.lib.optionals pkgs.stdenv.isLinux [
              desktopItem
            ];

            postInstall = pkgs.lib.optionalString pkgs.stdenv.isLinux ''
              wrapProgram $out/bin/frext \
                --prefix LD_LIBRARY_PATH : ${runtimeLibPath} \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.zenity ]}
            '';
          };

          default = self.packages.${system}.frext;
        }
      );

      ##########################################################################
      ## CHECKS — unified base+rust via mkCheck
      ##########################################################################
      checks = builtins.listToAttrs (
        map (
          system:
          let
            pkgs = import nixpkgs { inherit system; };

            gitHooks = precommit.inputs.git-hooks;

            extraExcludes = [
              "Cargo.lock"
            ];

            baseModule = precommit.lib.mkBaseCheck { inherit system extraExcludes; };
            rustModule = precommit.lib.mkRustCheck {
              inherit system extraExcludes;
              enableXtask = false;
            };

            tombiHook = {
              tombi = {
                enable = true;
                name = "tombi (TOML lint)";
                description = "Lint TOML files with tombi, failing on warnings.";
                entry = "${pkgs.tombi}/bin/tombi lint --error-on-warnings";
                files = "\\.toml$";
                language = "system";
                pass_filenames = true;
              };
            };

            mergedHooks = baseModule.hooks // rustModule.hooks // tombiHook;
            mergedExcludes = (baseModule.excludes or [ ]) ++ (rustModule.excludes or [ ]) ++ extraExcludes;

            run = gitHooks.lib.${system}.run {
              src = ./.;
              hooks = mergedHooks;
              excludes = mergedExcludes;
            };
          in
          {
            name = system;
            value = {
              pre-commit-check = run // {
                passthru = {
                  devPackages = (baseModule.passthru.devPackages or [ ]) ++ (rustModule.passthru.devPackages or [ ]);
                  libPath = (baseModule.passthru.libPath or [ ]) ++ (rustModule.passthru.libPath or [ ]);
                };
                shellHook = run.shellHook or "";
                enabledPackages = run.enabledPackages or [ ];
              };
            };
          }
        ) systems
      );

      ##########################################################################
      ## DEV SHELLS — merged env + extra Rust goodies
      ##########################################################################
      devShells = builtins.listToAttrs (
        map (system: {
          name = system;

          value =
            let
              pkgs = import nixpkgs { inherit system; };

              chk = self.checks.${system}."pre-commit-check";

              corePkgs = chk.enabledPackages or [ ];

              ciRustTools = [
                pkgs.tombi
                pkgs.cargo-deny
                pkgs.cargo-machete
                pkgs.typos
                pkgs.markdownlint-cli2
              ]
              ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
                pkgs.cargo-llvm-cov
              ];

              extraDev = chk.passthru.devPackages or [ ];

              # Runtime tools the editor shells out to. `rfd` uses zenity as
              # its file-dialog fallback when no XDG desktop portal FileChooser
              # backend is available on the session.
              runtimeTools = pkgs.lib.optionals pkgs.stdenv.isLinux [
                pkgs.zenity
              ];

              libPkgs =
                (chk.passthru.libPath or [ ])
                ++ [
                  pkgs.libGL
                  pkgs.libxkbcommon
                ]
                ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
                  pkgs.dbus.lib
                  pkgs.wayland
                ];

              mkFrextShell =
                extraTools:
                pkgs.mkShell {
                  buildInputs = extraDev ++ corePkgs ++ ciRustTools ++ runtimeTools ++ extraTools;

                  nativeBuildInputs = [ pkgs.pkg-config ];

                  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libPkgs;

                  shellHook = ''
                    ${chk.shellHook}

                    alias pre-commit="pre-commit run --all-files"
                  '';
                };
            in
            {
              default = mkFrextShell [ ];
              ci = mkFrextShell [ ];
            };
        }) systems
      );
    };
}
