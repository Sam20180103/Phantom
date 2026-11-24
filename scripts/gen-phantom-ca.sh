#!/usr/bin/env bash
#
# Create the private CA that signs every Phantom host certificate.
#
# Why bother, when `gen-phantom-cert.sh` can self-sign? Because a self-signed
# certificate has to be installed on every machine that talks to that host, and
# reinstalled the day the certificate changes — when it expires, or when the hub
# moves to a different box. With a CA, each machine trusts the root *once* and
# every certificate you issue afterwards is accepted with no further visits.
#
# Run this on exactly one machine and keep the key there. It is the mesh's
# master key: anyone holding it can impersonate any host in it.
#
# Usage:
#   scripts/gen-phantom-ca.sh [output-dir]
#
# output-dir defaults to ~/.config/phantom. Writes ca.pem and ca-key.pem, then
# prints how to trust the root on each platform.

set -euo pipefail

out_dir="${1:-${XDG_CONFIG_HOME:-$HOME/.config}/phantom}"
mkdir -p "$out_dir"
ca_cert="$out_dir/ca.pem"
ca_key="$out_dir/ca-key.pem"

# Refuse to clobber. Overwriting the root silently invalidates every
# certificate already issued from it and every machine already trusting it —
# a mesh-wide outage from one careless rerun.
if [ -e "$ca_cert" ] || [ -e "$ca_key" ]; then
  echo "a CA already exists in $out_dir" >&2
  echo "  $ca_cert" >&2
  echo "  $ca_key" >&2
  echo >&2
  echo "Reissuing the root invalidates every certificate signed by it and every" >&2
  echo "machine that trusts it. Move the old pair aside first if that is what" >&2
  echo "you intend." >&2
  exit 1
fi

# `pathlen:0` lets this root sign server certificates but no further CAs, so a
# leaked key cannot be used to mint an intermediate and hide behind it.
# Ten years: rotating a root means revisiting every machine, which is the one
# thing this exists to avoid.
openssl req -x509 -newkey rsa:4096 -sha256 -days 3650 -nodes \
  -keyout "$ca_key" -out "$ca_cert" \
  -subj "/CN=Phantom Mesh CA" \
  -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
  -addext "keyUsage=critical,keyCertSign,cRLSign"

chmod 600 "$ca_key"
chmod 644 "$ca_cert"

cat <<MSG

wrote $ca_cert   (public — copy this to every machine)
wrote $ca_key    (private — keep it here, never copy it)

Next:
  1. Issue this host's certificate from the CA:
       scripts/gen-phantom-cert.sh

  2. Trust the root on every machine that connects to a Phantom server.
     A browser can be clicked through; a Tauri webview cannot, so on macOS
     this is the only way the desktop app will connect at all.

       macOS
         sudo security add-trusted-cert -d -r trustRoot \\
           -k /Library/Keychains/System.keychain ca.pem

       Arch
         sudo cp ca.pem /etc/ca-certificates/trust-source/anchors/phantom-ca.crt
         sudo trust extract-compat

       Debian / Ubuntu
         sudo cp ca.pem /usr/local/share/ca-certificates/phantom-ca.crt
         sudo update-ca-certificates

  3. On peer servers, point the hub link at the root rather than at the hub's
     own certificate — then moving the hub needs no config change anywhere:
       room:
         hub:
           ca: <path to ca.pem>
MSG
