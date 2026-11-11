#!/usr/bin/env bash
# Installs the system libraries GPUI links against on the Linux runner.
# The runner's apt mirror list (azure.archive.ubuntu.com, then archive and
# security.ubuntu.com) went unresolvable from GitHub's runners on 2026-11-11
# while other hosts resolved, so a failed attempt retries once through a
# mirror outside ubuntu.com.
set -euo pipefail

packages=(libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libvulkan-dev libfontconfig-dev)

install() {
  sudo apt-get update
  sudo apt-get install -y "${packages[@]}"
}

if ! install; then
  echo "::warning::Ubuntu mirrors unreachable; retrying through mirrors.edge.kernel.org"
  echo "https://mirrors.edge.kernel.org/ubuntu/" | sudo tee /etc/apt/apt-mirrors.txt
  install
fi
