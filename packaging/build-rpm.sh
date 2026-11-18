#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
name=gnome-foreground-booster
version=$(rpmspec -q --srpm --qf '%{VERSION}' "packaging/$name.spec")
test "$version" = "$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"

stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
top=$stage/rpmbuild
src=$stage/$name-$version
mkdir -p "$src" "$top/SOURCES" "$top/SPECS"
{
    git ls-files -z --cached -- . ':!packaging'
    printf '%s\0' "packaging/$name.spec" packaging/build-rpm.sh
} |
    tar --null -T - -cf - | tar -xf - -C "$src"

(
    cd "$src"
    cargo vendor --locked vendor >/dev/null
    for dir in vendor/*/; do
        for file in "$dir"LICENSE* "$dir"COPYING* "$dir"UNLICENSE* "$dir"NOTICE*; do
            [ -f "$file" ] || continue
            printf '\n==== %s: %s ====\n\n' "$dir" "$(basename "$file")"
            cat "$file"
        done
    done > LICENSE.dependencies
)

tar -C "$stage" --exclude="$name-$version/vendor" -czf "$top/SOURCES/$name-$version.tar.gz" "$name-$version"
tar -C "$src" -cJf "$top/SOURCES/$name-$version-vendor.tar.xz" vendor
cp "$src/packaging/$name.spec" "$top/SPECS/"
rpmbuild --define "_topdir $top" -ba "$top/SPECS/$name.spec"

out=$PWD/dist/$version
mkdir -p "$out"
cp "$top"/{RPMS/*/*.rpm,SRPMS/*.rpm,SOURCES/*.tar.*} "$out/"
(cd "$out" && sha256sum ./*.rpm ./*.tar.* > SHA256SUMS)
