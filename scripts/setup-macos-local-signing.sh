#!/bin/sh
set -eu
umask 077

if [ "$(uname -s)" != Darwin ]; then
  echo "This setup must run on the Mac that will build/sign Talking Quill." >&2
  exit 1
fi

identity="Talking Quill Local Code Signing"
config_dir="$HOME/Library/Application Support/Talking Quill Local Build"
config="$config_dir/signing.json"
public_cert="$config_dir/Talking-Quill-Local-Code-Signing.cer"
login_keychain="$HOME/Library/Keychains/login.keychain-db"
mkdir -p "$config_dir" "$(pwd)/tmp"
chmod 700 "$config_dir"
temporary="$(mktemp -d "$(pwd)/tmp/macos-local-signing.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

identity_lines="$(/usr/bin/security find-identity -v -p codesigning "$login_keychain" | /usr/bin/grep -F "\"$identity\"" || true)"
identity_count="$(printf '%s\n' "$identity_lines" | /usr/bin/awk 'NF { count++ } END { print count+0 }')"
if [ "$identity_count" -gt 1 ]; then
  echo "More than one Keychain code-signing identity is named $identity; remove duplicates before continuing." >&2
  exit 1
fi

if [ "$identity_count" -eq 0 ] && [ -f "$config" ]; then
  echo "The saved identity is missing from the login Keychain. Restore it; deleting signing.json changes installation identity and permissions." >&2
  exit 1
fi

if [ "$identity_count" -eq 0 ]; then
  cat >"$temporary/openssl.cnf" <<EOF
[req]
prompt = no
distinguished_name = subject
x509_extensions = extensions
[subject]
CN = $identity
[extensions]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = codeSigning
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always
EOF

  /usr/bin/openssl req -new -x509 -newkey rsa:3072 -nodes -days 3650 \
    -config "$temporary/openssl.cnf" -keyout "$temporary/key.pem" -out "$temporary/cert.pem"
  password="$(/usr/bin/openssl rand -hex 24)"
  /usr/bin/openssl pkcs12 -export -name "$identity" -inkey "$temporary/key.pem" \
    -in "$temporary/cert.pem" -out "$temporary/identity.p12" -passout "pass:$password"
  /usr/bin/security import "$temporary/identity.p12" -k "$login_keychain" -P "$password" \
    -T /usr/bin/codesign -T /usr/bin/security
  /usr/bin/security add-trusted-cert -r trustAsRoot -k "$login_keychain" "$temporary/cert.pem"
  identity_lines="$(/usr/bin/security find-identity -v -p codesigning "$login_keychain" | /usr/bin/grep -F "\"$identity\"" || true)"
  identity_count="$(printf '%s\n' "$identity_lines" | /usr/bin/awk 'NF { count++ } END { print count+0 }')"
  if [ "$identity_count" -ne 1 ]; then
    echo "The imported identity is not uniquely usable for code signing." >&2
    exit 1
  fi
fi

/usr/bin/security find-certificate -c "$identity" -p "$login_keychain" >"$temporary/cert.pem"
if ! /usr/bin/openssl x509 -in "$temporary/cert.pem" -checkend 2592000 -noout; then
  echo "The local identity expires in less than 30 days; rotate it deliberately and re-enroll permissions." >&2
  exit 1
fi
certificate_text="$(/usr/bin/openssl x509 -in "$temporary/cert.pem" -text -noout)"
printf '%s\n' "$certificate_text" | /usr/bin/grep -F 'CA:FALSE' >/dev/null
printf '%s\n' "$certificate_text" | /usr/bin/grep -F 'Digital Signature' >/dev/null
printf '%s\n' "$certificate_text" | /usr/bin/grep -F 'Code Signing' >/dev/null
/usr/bin/security add-trusted-cert -r trustAsRoot -k "$login_keychain" "$temporary/cert.pem"
/usr/bin/security verify-cert -c "$temporary/cert.pem" -p basic >/dev/null
/usr/bin/security verify-cert -c "$temporary/cert.pem" -p codesigning >/dev/null
cat >"$temporary/probe" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod 700 "$temporary/probe"
/usr/bin/codesign --force --sign "$identity" --timestamp=none "$temporary/probe"
/usr/bin/codesign --verify --strict "$temporary/probe"
printf 'talking-quill-local-cms-probe\n' >"$temporary/cms-input"
/usr/bin/security cms -S -N "$identity" -i "$temporary/cms-input" -o "$temporary/cms-signature"
/usr/bin/security cms -D -i "$temporary/cms-signature" -o "$temporary/cms-output"
/usr/bin/cmp "$temporary/cms-input" "$temporary/cms-output"

sha256="$(/usr/bin/openssl x509 -in "$temporary/cert.pem" -outform DER | /usr/bin/shasum -a 256 | /usr/bin/awk '{print tolower($1)}')"
sha1="$(/usr/bin/openssl x509 -in "$temporary/cert.pem" -outform DER | /usr/bin/shasum -a 1 | /usr/bin/awk '{print tolower($1)}')"
identity_sha1="$(printf '%s\n' "$identity_lines" | /usr/bin/awk 'NF { print tolower($2) }')"
if [ "$sha1" != "$identity_sha1" ]; then
  echo "The Keychain identity and public certificate fingerprints disagree." >&2
  exit 1
fi

if [ -f "$config" ]; then
  expected="$(TQ_CONFIG="$config" /usr/bin/env node - <<'NODE'
const value = require(process.env.TQ_CONFIG);
process.stdout.write(`${value.TALKING_QUILL_MACOS_LOCAL_CERT_SHA256}\n${value.TALKING_QUILL_MACOS_LOCAL_CERT_SHA1}\n`);
NODE
)"
  expected_sha256="$(printf '%s\n' "$expected" | /usr/bin/sed -n '1p')"
  expected_sha1="$(printf '%s\n' "$expected" | /usr/bin/sed -n '2p')"
  if [ "$sha256" != "$expected_sha256" ] || [ "$sha1" != "$expected_sha1" ]; then
    echo "The saved signing pins do not match the unique usable Keychain identity." >&2
    exit 1
  fi
  /usr/bin/ditto "$temporary/cert.pem" "$public_cert"
  chmod 644 "$public_cert"
  echo "Reusing the validated stable Keychain identity and $config"
  exit 0
fi

installation_id="$(/usr/bin/openssl rand -hex 32)"
temporary_config="$temporary/signing.json"
TQ_CONFIG="$temporary_config" TQ_IDENTITY="$identity" TQ_SHA256="$sha256" TQ_SHA1="$sha1" TQ_INSTALLATION="$installation_id" \
/usr/bin/env node - <<'NODE'
const { writeFileSync } = require('node:fs');
const value = {
  TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256: process.env.TQ_SHA256,
  TALKING_QUILL_MACOS_POLICY_CMS_IDENTITY: process.env.TQ_IDENTITY,
  TALKING_QUILL_MACOS_INSTALLATION_ID: process.env.TQ_INSTALLATION,
  TALKING_QUILL_MACOS_LOCAL_IDENTITY: process.env.TQ_IDENTITY,
  TALKING_QUILL_MACOS_LOCAL_CERT_SHA256: process.env.TQ_SHA256,
  TALKING_QUILL_MACOS_LOCAL_CERT_SHA1: process.env.TQ_SHA1,
};
writeFileSync(process.env.TQ_CONFIG, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
NODE
mv "$temporary_config" "$config"
/usr/bin/ditto "$temporary/cert.pem" "$public_cert"
chmod 644 "$public_cert"
echo "Created stable local signing setup at $config"
echo "The public friend-Mac certificate is $public_cert"
echo "Keep this Keychain identity and signing.json stable for updates and permission continuity."
