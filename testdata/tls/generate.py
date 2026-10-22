#!/usr/bin/env python3
"""Generate TEST certificates for src/monitors/tls.rs (self-signed, no value).

Run from the repository root: python3 testdata/tls/generate.py
Requires the Python 'cryptography' package.
"""
import datetime
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

OUT = Path(__file__).parent


def cert(name, not_after, key):
    subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    builder = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(subject)
        .public_key(key.public_key())
        .serial_number(4711)
        .not_valid_before(datetime.datetime(2026, 1, 1, tzinfo=datetime.timezone.utc))
        .not_valid_after(not_after)
        .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost")]), critical=False)
    )
    der = builder.sign(key, hashes.SHA256()).public_bytes(serialization.Encoding.DER)
    (OUT / name).write_bytes(der)


key = ec.derive_private_key(0x1234567890ABCDEF, ec.SECP256R1())
(OUT / "key.pkcs8.der").write_bytes(
    key.private_bytes(
        serialization.Encoding.DER,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
)
# UTCTime (years before 2050) and GeneralizedTime (2050 and later).
cert("utctime.der", datetime.datetime(2030, 1, 2, 3, 4, 5, tzinfo=datetime.timezone.utc), key)
cert("generalized.der", datetime.datetime(2051, 6, 1, 12, 0, 0, tzinfo=datetime.timezone.utc), key)
