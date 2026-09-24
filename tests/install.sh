#!/bin/sh
set -eu
binary=$1
installer=$2
test_home=$(mktemp -d)
trap 'rm -rf "$test_home"' EXIT HUP INT TERM
for shell in zsh bash sh; do
    home="$test_home/$shell"
    mkdir -p "$home"
    if [ "$shell" = bash ]; then printf '# existing login file\n' > "$home/.bash_profile"; fi
    HOME="$home" SHELL="/bin/$shell" ZDOTDIR="$home/custom-zsh" sh "$installer" "$binary"
    HOME="$home" SHELL="/bin/$shell" ZDOTDIR="$home/custom-zsh" sh "$installer" "$binary"
    case "$shell" in
        zsh) rc="$home/custom-zsh/.zshrc" ;;
        bash) rc="$home/.bashrc"
            test "$(grep -Fc 'export PATH="$HOME/.local/bin:$PATH"' "$home/.bash_profile")" = 1 ;;
        sh) rc="$home/.profile" ;;
    esac
    test "$(grep -Fc 'export PATH="$HOME/.local/bin:$PATH"' "$rc")" = 1
    HOME="$home" sh -c '. "$1"; command -v ninko; command -v Ninko; ninko --version; Ninko --version' sh "$rc"
done
# A conflicting alias must leave the existing binary untouched (case-sensitive FS).
home="$test_home/conflict"
mkdir -p "$home/.local/bin"
printf old > "$home/.local/bin/ninko"
if [ ! -e "$home/.local/bin/Ninko" ]; then
    printf unrelated > "$home/.local/bin/Ninko"
    if HOME="$home" SHELL=/bin/sh sh "$installer" "$binary"; then exit 1; fi
    test "$(cat "$home/.local/bin/ninko")" = old
fi
printf '%s\n' 'Installer passed: zsh, bash, POSIX shell, repeated install, alias conflict'
