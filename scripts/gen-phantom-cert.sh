#!/usr/bin/env bash
#
# Generate this host's TLS certificate for phantom-server.
#
# phantom-server serves HTTPS whenever `cert` and `cert-key` are set in its
# config (see crates/phantom-server/src/config.rs). A box that already runs
# code-server should point those keys at code-server's certificate instead of
# running this. Use this only when there is no certificate to reuse.
#
# If a CA exists in the output directory (see gen-phantom-ca.sh) the
# certificate is signed by it; otherwise it is self-signed, which is what this
# script has always done. The difference matters to everyone else: a CA-signed
# certificate is accepted by any machine that already trusts the root, while a
# self-signed one has to be installed on each of them by hand, and reinstalled
# every time it changes.
#
# Usage:
#   scripts/gen-phantom-cert.sh [output-dir]
#
# output-dir defaults to ~/.config/phantom. Writes cert.pem and key.pem there
# and prints the two lines to add to config.yaml.

set -euo pipefail

out_dir="${1:-${XDG_CONFIG_HOME:-$HOME/.config}/phantom}"
mkdir -p "$out_dir"
cert="$out_dir/cert.pem"
key="$out_dir/key.pem"

# Subject Alternative Names decide which hostnames/IPs the cert is valid for.
# A browser reaching the server by an address not listed here rejects it, so
# cover every way in: loopback, the hostname, every global IP on this box
# (LAN + Tailscale), and the Tailscale MagicDNS name.
dns_names=("localhost")
ip_addrs=("127.0.0.1" "::1")

host="$(hostname 2>/dev/null || true)"
[ -n "$host" ] && [ "$host" != "localhost" ] && dns_names+=("$host")

# Global-scope addresses on every interface. `ip` is Linux-only; skip cleanly
# elsewhere and rely on the explicit args a caller can still pass via SANS_EXTRA.
if command -v ip >/dev/null 2>&1; then
  while read -r addr; do
    ip_addrs+=("${addr%%/*}")
  done < <(ip -o addr show scope global 2>/dev/null | awk '{print $4}')
fi

# Tailscale's own view, in case the interface address is filtered or the
# MagicDNS name is how devices actually reach the box.
if command -v tailscale >/dev/null 2>&1; then
  while read -r addr; do
    [ -n "$addr" ] && ip_addrs+=("$addr")
  done < <(tailscale ip 2>/dev/null || true)
  ts_dns="$(tailscale status --json 2>/dev/null \
    | grep -o '"DNSName": *"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/; s/\.$//')"
  [ -n "$ts_dns" ] && dns_names+=("$ts_dns")
fi

# Anything extra the caller wants, comma-separated, already in openssl form
# (e.g. SANS_EXTRA="DNS:phantom.example.com,IP:10.0.0.5").
san="$(
  {
    printf 'DNS:%s\n' "${dns_names[@]}"
    printf 'IP:%s\n' "${ip_addrs[@]}"
    if [ -n "${SANS_EXTRA:-}" ]; then printf '%s\n' "${SANS_EXTRA//,/$'\n'}"; fi
  } | awk 'NF && !seen[$0]++' | paste -sd,
)"

# 825 days either way: browsers reject a server certificate valid for longer,
# CA-signed or not.
ca_cert="${CA_DIR:-$out_dir}/ca.pem"
ca_key="${CA_DIR:-$out_dir}/ca-key.pem"

# The extensions are identical in both branches — only the signer differs.
exts="subjectAltName=$san
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth"

if [ -r "$ca_cert" ] && [ -r "$ca_key" ]; then
  signed_by="the CA in ${CA_DIR:-$out_dir}"
  csr="$(mktemp)"
  trap 'rm -f "$csr"' EXIT
  openssl req -new -newkey rsa:2048 -nodes \
    -keyout "$key" -out "$csr" \
    -subj "/CN=$(hostname 2>/dev/null || echo phantom-server)" >/dev/null 2>&1
  openssl x509 -req -in "$csr" -CA "$ca_cert" -CAkey "$ca_key" -CAcreateserial \
    -days 825 -sha256 -out "$cert" \
    -extfile <(printf '%s\n' "$exts") >/dev/null 2>&1
else
  signed_by="itself (no CA found — see scripts/gen-phantom-ca.sh)"
  openssl req -x509 -newkey rsa:2048 -sha256 -days 825 -nodes \
    -keyout "$key" -out "$cert" \
    -subj "/CN=phantom-server" \
    -addext "subjectAltName=$san" \
    -addext "basicConstraints=critical,CA:FALSE" \
    -addext "keyUsage=critical,digitalSignature,keyEncipherment" \
    -addext "extendedKeyUsage=serverAuth"
fi

chmod 600 "$key"

echo
echo "wrote $cert"
echo "wrote $key"
echo "signed by: $signed_by"
echo "SANs: $san"
echo
echo "add to $out_dir/config.yaml:"
echo "  cert: $cert"
echo "  cert-key: $key"
