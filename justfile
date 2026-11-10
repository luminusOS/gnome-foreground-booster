prefix := env_var_or_default("PREFIX", "/usr")
unitdir := env_var_or_default("UNITDIR", prefix + "/lib/systemd/user")

check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

build:
    cargo build --release
    mkdir -p build
    cp "$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/release/gnome-foreground-booster" build/

install:
    test -x build/gnome-foreground-booster || { echo "run 'just build' first" >&2; exit 1; }
    install -Dm755 build/gnome-foreground-booster {{prefix}}/bin/gnome-foreground-booster
    mkdir -p {{unitdir}}
    sed 's|@BINDIR@|{{prefix}}/bin|' data/gnome-foreground-booster.service.in > {{unitdir}}/gnome-foreground-booster.service
    chmod 644 {{unitdir}}/gnome-foreground-booster.service

install-user: build
    PREFIX=$HOME/.local UNITDIR=$HOME/.config/systemd/user just install

uninstall:
    rm -f {{prefix}}/bin/gnome-foreground-booster
    rm -f {{unitdir}}/gnome-foreground-booster.service

e2e:
    tests/e2e/run.sh
