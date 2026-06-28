# Overlay that adds the `frext` package to nixpkgs.
#
# Usage in a flake:
#   nixpkgs.overlays = [ frext.overlays.default ];
#
# Then `pkgs.frext` is available.
{ frext-flake }:
final: _prev: {
  inherit (frext-flake.packages.${final.stdenv.hostPlatform.system}) frext;
}
