#!/usr/bin/env sh
# Generate a local CA and one server certificate covering every hostname this
# server impersonates — by hand, with openssl.
#
# The server does this itself now: with AMS_CLIENTS_BIND set it makes its own
# authority under AMS_TLS_DIR and serves it at /ca.crt. This script remains
# for whoever would rather run an authority of their own and hand the result
# to AMS_CLIENTS_TLS_CERT / AMS_CLIENTS_TLS_KEY.
#
# Sonarr, Radarr and TMDB clients have their metadata URLs compiled in, so the
# only way to reach a different server is to resolve those hostnames to this
# one. They are HTTPS, so this server must present a certificate the client
# trusts — hence a CA you add to each client's trust store.
#
# Produces, next to this script:
#   ca.crt, ca.key          the local CA. ca.crt is what clients must trust.
#   server.crt, server.key  what the metadata server presents.
#
# ca.key is the private key of an authority your stack trusts. Keep it off
# shared storage and out of version control.

set -eu

DIR="$(cd "$(dirname "$0")" && pwd)/certs"
mkdir -p "$DIR"
cd "$DIR"

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl is required" >&2
  exit 1
fi

# Every hostname a client might have compiled in.
HOSTS="skyhook.sonarr.tv api.radarr.video api.themoviedb.org"

if [ -f ca.key ]; then
  echo "Reusing the existing CA in $DIR"
else
  echo "Creating a CA in $DIR"
  openssl req -x509 -newkey rsa:4096 -sha256 -days 3650 -nodes \
    -keyout ca.key -out ca.crt \
    -subj "/CN=arr-metadata-server local CA" \
    -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
    -addext "keyUsage=critical,keyCertSign,cRLSign"
fi

SAN=""
for host in $HOSTS; do
  SAN="${SAN}DNS:${host},"
done
SAN="${SAN}DNS:localhost,IP:127.0.0.1"

echo "Issuing a server certificate for: $HOSTS"

openssl req -newkey rsa:2048 -sha256 -nodes \
  -keyout server.key -out server.csr \
  -subj "/CN=arr-metadata-server"

cat > ext.cnf <<EXT
subjectAltName=${SAN}
extendedKeyUsage=serverAuth
basicConstraints=CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
EXT

openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -out server.crt -days 825 -sha256 -extfile ext.cnf

rm -f server.csr ext.cnf ca.srl
chmod 600 ca.key server.key

echo
echo "Done. In $DIR:"
ls -1 ca.crt ca.key server.crt server.key
echo
echo "  server.crt / server.key  → AMS_CLIENTS_TLS_CERT / AMS_CLIENTS_TLS_KEY on the metadata server"
echo "  ca.crt                   → add to each client's trust store:"
echo "      Node clients (Jellyseerr, Overseerr): NODE_EXTRA_CA_CERTS=/path/ca.crt"
echo "      .NET clients (Sonarr, Radarr):        mount into /usr/local/share/ca-certificates"
echo "                                            and run update-ca-certificates"
