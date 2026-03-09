# Fumi home-manager module — GPU-rendered multi-protocol chat client
#
# Namespace: programs.fumi
#
# Module factory: receives { hmHelpers } from flake.nix, returns HM module.
{ hmHelpers }:
{
  lib,
  config,
  pkgs,
  ...
}:
with lib;
let
  cfg = config.programs.fumi;
in
{
  options.programs.fumi = {
    enable = mkOption {
      type = types.bool;
      default = false;
      description = "Enable the fumi multi-protocol chat client.";
    };
    package = mkOption {
      type = types.package;
      default = pkgs.fumi;
      description = "The fumi package to install.";
    };
  };
  config = mkIf cfg.enable {
    home.packages = [ cfg.package ];
  };
}
