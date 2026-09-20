#!/usr/bin/env bash
# Install this server's CA into a linuxserver.io container's trust store.
#
# Mount this at /custom-cont-init.d/ alongside the CA itself at
# /usr/local/share/ca-certificates/. The image runs everything executable in
# that directory before starting its service, which is the only hook these
# images offer — they do NOT run update-ca-certificates on their own, and
# dropping a certificate into the directory alone has no effect.
#
# Sonarr and Radarr are .NET on Alpine, so they read the OpenSSL bundle that
# update-ca-certificates rebuilds.

set -eu

CA_NAME="arr-metadata.crt"
CA_PATH="/usr/local/share/ca-certificates/${CA_NAME}"

if [ ! -f "${CA_PATH}" ]; then
    echo "[trust-ca] ${CA_PATH} is not mounted; nothing to install"
    exit 0
fi

update-ca-certificates >/dev/null 2>&1

# The bundle is a bare concatenation of PEM blocks — no subject lines to match
# on, and these images carry no openssl to decode with. Probe with a line of the
# certificate's own base64 body instead.
PROBE="$(sed -n '3p' "${CA_PATH}")"

if [ -n "${PROBE}" ] && grep -qF "${PROBE}" /etc/ssl/certs/ca-certificates.crt 2>/dev/null; then
    echo "[trust-ca] CA installed; this container now trusts the metadata server"
else
    echo "[trust-ca] WARNING: the CA did not land in the trust bundle" >&2
    exit 1
fi
