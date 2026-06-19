"""
Cryptographic signing utilities for secure token generation.
"""

import hashlib
import hmac
import time
from typing import Optional


class Signer:
    """Generates and verifies signed tokens using HMAC."""

    def __init__(self, secret_key: str, algorithm: str = "sha256"):
        self.secret_key = secret_key.encode("utf-8")
        self.algorithm = algorithm

    def sign(self, value: str) -> str:
        """Create a signed token from a value."""
        timestamp = str(int(time.time()))
        payload = f"{value}.{timestamp}"
        signature = self._compute_signature(payload)
        return f"{payload}.{signature}"

    def unsign(self, signed_value: str, max_age: Optional[int] = None) -> str:
        """Verify and extract the original value from a signed token."""
        parts = signed_value.rsplit(".", 2)
        if len(parts) != 3:
            raise ValueError("Invalid signed value format")

        value, timestamp, signature = parts

        # Verify signature
        expected_payload = f"{value}.{timestamp}"
        expected_signature = self._compute_signature(expected_payload)

        if not hmac.compare_digest(signature, expected_signature):
            raise ValueError("Invalid signature")

        # Check expiration
        if max_age is not None:
            age = int(time.time()) - int(timestamp)
            if age > max_age:
                raise ValueError(f"Token expired (age: {age}s, max: {max_age}s)")

        return value

    def _compute_signature(self, payload: str) -> str:
        """Compute HMAC signature for a payload."""
        h = hmac.new(self.secret_key, payload.encode("utf-8"), self.algorithm)
        return h.hexdigest()
