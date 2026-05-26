#!/usr/bin/env bash
# Install cosmic-fabric Phase-1: the daemon (cosmic-fabricd + core) and the
# thin launcher plugin. User-dir install — no sudo.
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIBDIR="${HOME}/.local/share/cosmic-fabric"
PLUGIN_DIR="${HOME}/.local/share/pop-launcher/plugins/cosmic-fabric"
CONFIG_DIR="${HOME}/.config/cosmic-fabric"

echo "Installing daemon → ${LIBDIR}"
mkdir -p "${LIBDIR}"
install -m 0644 "${SRC}/core.py"        "${LIBDIR}/core.py"
install -m 0755 "${SRC}/cosmic-fabricd" "${LIBDIR}/cosmic-fabricd"

echo "Installing launcher plugin → ${PLUGIN_DIR}"
mkdir -p "${PLUGIN_DIR}"
install -m 0755 "${SRC}/cosmic-fabric-launcher" "${PLUGIN_DIR}/cosmic-fabric-launcher"
install -m 0644 "${SRC}/plugin.ron"             "${PLUGIN_DIR}/plugin.ron"

if [ ! -f "${CONFIG_DIR}/policy.toml" ]; then
    echo "Seeding ${CONFIG_DIR}/policy.toml"
    mkdir -p "${CONFIG_DIR}"
    install -m 0644 "${SRC}/../prototype/policy.toml.example" "${CONFIG_DIR}/policy.toml" 2>/dev/null || true
fi

echo
echo "Done. The launcher auto-spawns cosmic-fabricd on first use (it ensures"
echo "'fabric --serve' is running too). To pick up changes:"
echo "    pkill cosmic-fabricd ; pkill cosmic-launcher"
echo "Then open the launcher and type:  fab summ"
echo "Daemon log: ~/.cache/cosmic-fabric/daemon.log"
