# Home-manager module for the frext text editor.
#
# Usage (in your home-manager config):
#
#   imports = [ frext.homeManagerModules.default ];
#
#   programs.frext = {
#     enable = true;
#     settings = {
#       theme = "catppuccin-mocha";
#     };
#   };
{ frext-flake }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkOption
    mkIf
    types
    ;
  cfg = config.programs.frext;

  # Resolve the package from the flake directly — no overlay required.
  defaultPackage = frext-flake.packages.${pkgs.stdenv.hostPlatform.system}.frext;

  tomlFormat = pkgs.formats.toml { };
in
{
  options.programs.frext = {
    enable = mkEnableOption "frext, a super lightweight text editor";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "frext.packages.\${system}.frext";
      description = "The frext package to install.";
    };

    settings = mkOption {
      inherit (tomlFormat) type;
      default = { };
      example = lib.literalExpression ''
        {
          theme = "catppuccin-mocha";
        }
      '';
      description = ''
        Configuration written verbatim to
        {file}`$XDG_CONFIG_HOME/frext/config.toml`. When the attrset is empty
        no file is written, so frext falls back to its built-in defaults.
      '';
    };
  };

  config = mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."frext/config.toml" = mkIf (cfg.settings != { }) {
      source = tomlFormat.generate "frext-config.toml" cfg.settings;
    };
  };
}
