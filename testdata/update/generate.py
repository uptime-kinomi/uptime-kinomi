#!/usr/bin/env python3
"""Generate minisign test vectors for src/update.rs.

The keys are deterministic TEST keys (fixed seeds) and have no relation to
the real release keys in keys/. Requires the Python 'cryptography' package.
Run from the repository root: python3 testdata/update/generate.py
"""
import base64
import hashlib
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

OUT = Path(__file__).parent


def key(seed_byte, key_id):
    return Ed25519PrivateKey.from_private_bytes(bytes([seed_byte]) * 32), key_id


def public_key_file(k):
    sk, key_id = k
    pk = sk.public_key().public_bytes_raw()
    b64 = base64.b64encode(b"Ed" + key_id + pk).decode()
    return f"untrusted comment: minisign public key (TEST ONLY)\n{b64}\n"


def signature_file(k, data, trusted_comment, prehashed=True):
    sk, key_id = k
    message = hashlib.blake2b(data, digest_size=64).digest() if prehashed else data
    sig = sk.sign(message)
    algorithm = b"ED" if prehashed else b"Ed"
    global_sig = sk.sign(sig + trusted_comment.encode())
    return (
        "untrusted comment: signature from TEST key\n"
        f"{base64.b64encode(algorithm + key_id + sig).decode()}\n"
        f"trusted comment: {trusted_comment}\n"
        f"{base64.b64encode(global_sig).decode()}\n"
    )


test_key = key(0x11, bytes(range(1, 9)))
other_key = key(0x22, bytes(range(1, 9)))  # same key ID, different key

sums = "".join(
    f"{hashlib.sha256(name.encode()).hexdigest()}  {name}\n"
    for name in [
        "uptime-kinomi-aarch64",
        "uptime-kinomi-armv6",
        "uptime-kinomi-armv7",
        "uptime-kinomi-riscv64",
        "uptime-kinomi-x86_64",
    ]
).encode()

(OUT / "test.pub").write_text(public_key_file(test_key))
(OUT / "SHA256SUMS").write_bytes(sums)
(OUT / "good.minisig").write_text(signature_file(test_key, sums, "uptime-kinomi 1.2.3"))
(OUT / "wrong-comment.minisig").write_text(signature_file(test_key, sums, "other-tool 1.2.3"))
(OUT / "other-key.minisig").write_text(signature_file(other_key, sums, "uptime-kinomi 1.2.3"))
(OUT / "legacy.minisig").write_text(
    signature_file(test_key, sums, "uptime-kinomi 1.2.3", prehashed=False)
)
