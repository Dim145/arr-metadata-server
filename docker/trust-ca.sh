#!/usr/bin/env sh
# Make this container trust the metadata server's authority.
#
# Mount it at /custom-cont-init.d/ of a linuxserver.io image (Sonarr, Radarr,
# Prowlarr…): the image runs everything executable in that directory, as
# root, before its service starts — the only hook these images offer. Sonarr
# and Radarr are .NET, which reads the OpenSSL bundle update-ca-certificates
# rebuilds; dropping a certificate somewhere alone changes nothing.
#
# The authority's certificate is taken from the first of these that exists:
#   1. a file mounted at /usr/local/share/ca-certificates/arr-metadata.crt;
#   2. the server's data volume, mounted read-only: $AMS_CA_FILE, by default
#      /arr-metadata/tls/ca.crt (mount the volume at /arr-metadata);
#   3. the server itself: $AMS_CA_URL, e.g. http://metadata:8080/ca.crt. Over
#      plain HTTP, whatever answers would become a root this container
#      trusts for everything, so the script refuses unless what is fetched is
#      pinned with $AMS_CA_FINGERPRINT — the SHA-256 the administration page
#      shows — or the risk is accepted, on a network you control, with
#      AMS_CA_INSECURE=1.
#
# What is fetched, or pinned, is one certificate and nothing else: a file
# with a second PEM block is refused, since update-ca-certificates would
# install every block of it.
#
# The same script is served by the server at /trust-ca.sh, and its
# authority at /ca.crt, so a deployment from the image alone has both.
# Mounted on purpose and finding nothing to install, it fails loudly: a
# Sonarr that starts untrusted fails later, with less to go on.

set -eu

CA_NAME="arr-metadata.crt"
CA_PATH="/usr/local/share/ca-certificates/${CA_NAME}"
CA_FILE="${AMS_CA_FILE:-/arr-metadata/tls/ca.crt}"

# Set once the authority is fetched from AMS_CA_URL rather than found here.
FETCHED=""

if [ -s "${CA_PATH}" ]; then
    echo "[trust-ca] using the authority mounted at ${CA_PATH}"
elif [ -s "${CA_FILE}" ]; then
    echo "[trust-ca] taking the authority from ${CA_FILE}"
    mkdir -p "$(dirname "${CA_PATH}")"
    cp "${CA_FILE}" "${CA_PATH}"
elif [ -n "${AMS_CA_URL:-}" ]; then
    case "${AMS_CA_URL}" in
        [Hh][Tt][Tt][Pp]://*)
            if [ -z "${AMS_CA_FINGERPRINT:-}" ]; then
                # ASCII words alone are compared (true, yes, on), and a minimal
                # busybox `tr` may know no character classes.
                # shellcheck disable=SC2018,SC2019
                case "$(printf '%s' "${AMS_CA_INSECURE:-}" | tr 'A-Z' 'a-z')" in
                    1|true|yes|on)
                        echo "[trust-ca] WARNING: installing an authority fetched over plain HTTP without AMS_CA_FINGERPRINT (AMS_CA_INSECURE is set): anyone on the way could have substituted their own" >&2
                        ;;
                    *)
                        echo "[trust-ca] ERROR: ${AMS_CA_URL} is plain HTTP and AMS_CA_FINGERPRINT is not set: whatever answers would be trusted as a root. Set AMS_CA_FINGERPRINT to the SHA-256 the server's Opening & APIs page shows, mount the authority instead, or set AMS_CA_INSECURE=1 to accept the risk on a network you control" >&2
                        exit 1
                        ;;
                esac
            fi
            ;;
    esac
    echo "[trust-ca] fetching the authority from ${AMS_CA_URL}"
    FETCHED=1
    mkdir -p "$(dirname "${CA_PATH}")"
    if command -v curl >/dev/null 2>&1; then
        curl -fsS --retry 10 --retry-delay 3 --retry-connrefused -o "${CA_PATH}" "${AMS_CA_URL}"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "${CA_PATH}" "${AMS_CA_URL}"
    else
        echo "[trust-ca] neither curl nor wget is available to fetch it" >&2
        exit 1
    fi
else
    echo "[trust-ca] ERROR: no authority to install: mount it at ${CA_PATH}, mount the server's data volume at /arr-metadata, or set AMS_CA_URL" >&2
    exit 1
fi

if ! grep -q "BEGIN CERTIFICATE" "${CA_PATH}"; then
    echo "[trust-ca] ${CA_PATH} is not a PEM certificate" >&2
    exit 1
fi

# A file that was fetched, or that a fingerprint is given for, is one
# certificate. The fingerprint below is taken of the file's base64 text as one
# stream, and a decoder is more lenient than OpenSSL: GNU's stops at a tab that
# OpenSSL skips, so a second certificate behind one goes unhashed, and
# update-ca-certificates installs every block of the file all the same. Any
# kind of PEM marker is counted — a TRUSTED CERTIFICATE block is a root to
# OpenSSL too — and the one let through is a CERTIFICATE.
if [ -n "${FETCHED}" ] || [ -n "${AMS_CA_FINGERPRINT:-}" ]; then
    BLOCKS="$(grep -o -e '-----BEGIN' "${CA_PATH}" | wc -l | tr -d ' ')"
    if [ "${BLOCKS}" != 1 ] || ! grep -q -e '^-----BEGIN CERTIFICATE-----' "${CA_PATH}"; then
        echo "[trust-ca] ERROR: ${CA_PATH} holds ${BLOCKS} PEM blocks, or its one block is not a CERTIFICATE: an authority that is fetched or pinned is exactly one certificate, since a second block would be installed without being pinned; not installed" >&2
        rm -f "${CA_PATH}" 2>/dev/null || true
        exit 1
    fi
fi

# Pinned, when a fingerprint was given: the SHA-256 of the certificate's DER,
# as the administration page shows it, colons or not, in either case.
if [ -n "${AMS_CA_FINGERPRINT:-}" ]; then
    WANT="$(printf '%s' "${AMS_CA_FINGERPRINT}" | tr -d ': ' | tr 'A-F' 'a-f')"
    HAVE="$(sed -e '/-----/d' "${CA_PATH}" | tr -d '\n\r ' | base64 -d 2>/dev/null | sha256sum | cut -c1-64)"
    if [ "${WANT}" != "${HAVE}" ]; then
        echo "[trust-ca] ERROR: the authority's fingerprint is ${HAVE}, not the ${WANT} expected; not installed" >&2
        rm -f "${CA_PATH}" 2>/dev/null || true
        exit 1
    fi
fi

# Readable by the service; a file mounted read-only is already that.
chmod 644 "${CA_PATH}" 2>/dev/null || true

if ! OUT="$(update-ca-certificates 2>&1)"; then
    printf '%s\n' "${OUT}" >&2
    echo "[trust-ca] ERROR: update-ca-certificates failed" >&2
    exit 1
fi

# The bundle is a bare concatenation of PEM blocks — no subject lines to match
# on, and these images carry no openssl to decode with. Probe with a line of the
# certificate's own base64 body instead.
PROBE="$(sed -n '3p' "${CA_PATH}")"

if [ -n "${PROBE}" ] && grep -qF "${PROBE}" /etc/ssl/certs/ca-certificates.crt 2>/dev/null; then
    echo "[trust-ca] authority installed; this container now trusts the metadata server"
else
    echo "[trust-ca] WARNING: the authority did not land in the trust bundle" >&2
    exit 1
fi
