#!/bin/sh
# Install for this user, without sudo. Supports zsh, bash and POSIX login shells.
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
# Check aliases before replacing the existing installation.
if [ -e "$destination/Ninko" ] || [ -L "$destination/Ninko" ]; then
    if [ ! "$destination/Ninko" -ef "$destination/ninko" ] &&
       [ "$(readlink "$destination/Ninko" 2>/dev/null || true)" != ninko ]; then
        printf '%s\n' "Ninko 已存在且不是本程序，请检查 $destination/Ninko" >&2
        exit 1
    fi
fi
temporary=$(mktemp "$destination/.ninko.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp "$binary" "$temporary"
chmod 755 "$temporary"
mv -f "$temporary" "$destination/ninko"
# On the usual macOS case-insensitive filesystem, Ninko already resolves to ninko.
if [ ! -e "$destination/Ninko" ] && [ ! -L "$destination/Ninko" ]; then
    ln -s ninko "$destination/Ninko"
elif [ ! "$destination/Ninko" -ef "$destination/ninko" ]; then
    printf '%s\n' "Ninko 已存在且不是本程序，请检查 $destination/Ninko" >&2
    exit 1
fi
install_shell=${SHELL:-sh}
case "${install_shell##*/}" in
    zsh) rc="${ZDOTDIR:-$HOME}/.zshrc" ;;
    bash) rc="$HOME/.bashrc" ;;
    *) rc="$HOME/.profile" ;;
esac
path_line='export PATH="$HOME/.local/bin:$PATH"'
add_path() {
    if ! grep -Fq "$path_line" "$1" 2>/dev/null; then
        mkdir -p "$(dirname "$1")"
        if [ -f "$1" ]; then cp "$1" "$1.ninko-backup-$(date +%Y%m%d%H%M%S)"; fi
        printf '\n# Ninko\n%s\n' "$path_line" >> "$1"
    fi
}
add_path "$rc"
# bash login shells read only the first existing login file, not necessarily .profile.
if [ "${install_shell##*/}" = bash ]; then
    login_rc="$HOME/.profile"
    for candidate in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [ -f "$candidate" ]; then login_rc="$candidate"; break; fi
    done
    add_path "$login_rc"
fi
printf '%s\n' "已安装：$destination/ninko（也可输入 Ninko）" '当前终端如未找到命令，请执行：export PATH="$HOME/.local/bin:$PATH"'
