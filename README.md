# x11-ewmh-shim

A tiny shim I developed to make Dofus 3 work under xwayland-sattelite. No need to thank me, Ankama.

Fixes this error when launching the game:
```
Task dozt completed.
Task dozu completed.
Error: Exception has been thrown by the target of an invocation.
```

## Usage

### NixOS, with home-manager

```nix
inputs.x11-ewmh-shim.url = "github:yannhodiesne/x11-ewmh-shim";

# NixOS side
home-manager.sharedModules = [ inputs.x11-ewmh-shim.homeManagerModules.default ];

# home-manager side
x11-ewmh-shim.enable = true; # DISPLAY=:0, change with x11-ewmh-shim.display
```

### any distro with Nix

`nix run github:yannhodiesne/x11-ewmh-shim` then run the game.

### Other distributions

```sh
cargo install --path .
~/.cargo/bin/x11-ewmh-shim
```

Enable it per session by putting this in `~/.config/systemd/user/x11-ewmh-shim.service`:

```ini
[Unit]
Description=Publish the EWMH root properties xwayland-satellite leaves out
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Environment=DISPLAY=:0
ExecStart=%h/.cargo/bin/x11-ewmh-shim
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
```

Then run:
```sh
systemctl --user enable --now x11-ewmh-shim.service
```

## Development

```sh
cargo test
```

The tests needs `Xvfb`, ensure it is on `PATH`
