prefix := env_var_or_default("PREFIX", "/usr")
unitdir := env_var_or_default("UNITDIR", prefix + "/lib/systemd/user")

check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

build:
    cargo build --release

install: build
    install -Dm755 target/release/gnome-foreground-booster {{prefix}}/bin/gnome-foreground-booster
    mkdir -p {{unitdir}}
    sed 's|@BINDIR@|{{prefix}}/bin|' data/gnome-foreground-booster.service.in > {{unitdir}}/gnome-foreground-booster.service
    chmod 644 {{unitdir}}/gnome-foreground-booster.service

install-user:
    PREFIX=$HOME/.local UNITDIR=$HOME/.config/systemd/user just install

uninstall:
    rm -f {{prefix}}/bin/gnome-foreground-booster
    rm -f {{unitdir}}/gnome-foreground-booster.service
