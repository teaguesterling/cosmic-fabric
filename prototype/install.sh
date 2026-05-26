#!/usr/bin/env bash
# Install the Phase-0 cosmic-fabric launcher as a pop-launcher (COSMIC) plugin.
# User-dir install — no sudo.
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_DIR="${HOME}/.local/share/pop-launcher/plugins/cosmic-fabric"
CONFIG_DIR="${HOME}/.config/cosmic-fabric"

echo "Installing plugin → ${PLUGIN_DIR}"
mkdir -p "${PLUGIN_DIR}"
install -m 0755 "${SRC}/cosmic-fabric-launcher" "${PLUGIN_DIR}/cosmic-fabric-launcher"
install -m 0644 "${SRC}/plugin.ron"             "${PLUGIN_DIR}/plugin.ron"

echo "Seeding config → ${CONFIG_DIR}/policy.toml"
mkdir -p "${CONFIG_DIR}"
if [ -f "${CONFIG_DIR}/policy.toml" ]; then
    echo "  policy.toml exists — left untouched."
else
    install -m 0644 "${SRC}/policy.toml.example" "${CONFIG_DIR}/policy.toml"
fi

echo
echo "Done. The COSMIC launcher picks up new plugins on its next start."
echo "If it doesn't appear, restart the launcher service:"
echo "    pkill cosmic-launcher        # it respawns on next Super-press"
echo
echo "Then open the launcher and type:  fab summ"
echo "(Requires: fabric in ~/.local/bin, wl-clipboard, notify-send.)"
