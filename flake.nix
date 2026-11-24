{
  description = "Publish the EWMH root properties that xwayland-satellite leaves out";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs, ... }:
    let
      systems = [
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);

      shim =
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "x11-ewmh-shim";
          version = "0.1.0";
          # the tests needs Xvfb
          nativeBuildInputs = [ pkgs.xvfb ];
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type: baseNameOf path != "target" && pkgs.lib.cleanSourceFilter path type;
          };
          cargoHash = "sha256-MD+ilRw531NjyY2D6dlxcX7WyPMdpIPg6TiQJylBBqY=";
        };
    in
    {
      packages = forAllSystems (system: {
        default = shim system;
        x11-ewmh-shim = shim system;
      });

      overlays.default = final: prev: { x11-ewmh-shim = self.packages.${final.system}; };

      homeManagerModules.default =
        {
          config,
          pkgs,
          lib,
          ...
        }:
        {
          options.x11-ewmh-shim = {
            enable = lib.mkEnableOption "advertising the EWMH root properties xwayland-satellite leaves out";
            display = lib.mkOption {
              type = lib.types.str;
              default = ":0";
              example = ":1";
              description = "X display of the xwayland-satellite session to publish the properties on.";
            };
          };

          config = lib.mkIf config.x11-ewmh-shim.enable {
            home.packages = [ self.packages.${pkgs.stdenv.hostPlatform.system}.x11-ewmh-shim ];

            systemd.user.services.x11-ewmh-shim = {
              Unit = {
                Description = "Publish the EWMH root properties xwayland-satellite leaves out";
                PartOf = [ "graphical-session.target" ];
                After = [ "graphical-session.target" ];
              };
              Service = {
                Environment = "DISPLAY=${config.x11-ewmh-shim.display}";
                ExecStart = "${self.packages.${pkgs.stdenv.hostPlatform.system}.x11-ewmh-shim}/bin/x11-ewmh-shim";
                Restart = "on-failure";
                RestartSec = 2;
              };
              Install.WantedBy = [ "graphical-session.target" ];
            };
          };
        };

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-tree);
    };
}
