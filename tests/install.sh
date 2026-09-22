#!/bin/sh
set -eu
binary=$1
installer=$2
test_home=$(mktemp -d)
trap 'rm -rf "$test_home"' EXIT HUP INT TERM
HOME="$test_home" ZDOTDIR="$test_home" sh "$installer" "$binary"
HOME="$test_home" ZDOTDIR="$test_home" sh "$installer" "$binary"
test "$(grep -Fc 'export PATH="$HOME/.local/bin:$PATH"' "$test_home/.zshrc")" = 1
HOME="$test_home" ZDOTDIR="$test_home" zsh -ic 'source "$ZDOTDIR/.zshrc"; command -v ninko; command -v Ninko; ninko --version; Ninko --version'
