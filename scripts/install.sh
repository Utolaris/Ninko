#!/bin/sh
# Install for this user, without sudo. Works with zsh on macOS and Linux.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "$#" -gt 0 ]; then
    binary=$1
else
    cd "$project_dir"
    cargo build --release
    binary="$project_dir/target/release/ninko"
fi
destination="$HOME/.local/bin"
mkdir -p "$destination"
temporary=$(mktemp "$destination/.ninko.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp "$binary" "$temporary"
chmod 755 "$temporary"
mv -f "$temporary" "$destination/ninko"
# On the usual macOS case-insensitive filesystem, Ninko already resolves to ninko.
if [ ! -e "$destination/Ninko" ]; then
    ln -s ninko "$destination/Ninko"
elif [ ! "$destination/Ninko" -ef "$destination/ninko" ]; then
    printf '%s\n' "Ninko 已存在且不是本程序，请检查 $destination/Ninko" >&2
    exit 1
fi
rc="${ZDOTDIR:-$HOME}/.zshrc"
path_line='export PATH="$HOME/.local/bin:$PATH"'
if ! grep -Fq "$path_line" "$rc" 2>/dev/null; then
    if [ -f "$rc" ]; then cp "$rc" "$rc.ninko-backup-$(date +%Y%m%d%H%M%S)"; fi
    printf '\n# Ninko\n%s\n' "$path_line" >> "$rc"
fi
printf '%s\n' "已安装：$destination/ninko（也可输入 Ninko）" "已有终端如未找到命令，请执行：source $rc"
