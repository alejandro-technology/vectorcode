"""User authentication module."""

from dataclasses import dataclass
from typing import Optional
import hashlib
import secrets


@dataclass
class User:
    """Represents a user in the system."""
    
    username: str
    email: str
    password_hash: str
    is_active: bool = True
    
    def verify_password(self, password: str) -> bool:
        """Verify a password against the stored hash."""
        salt = self.password_hash[:32]
        expected = self._hash_password(password, salt)
        return secrets.compare_digest(self.password_hash, expected)
    
    @staticmethod
    def _hash_password(password: str, salt: Optional[str] = None) -> str:
        """Hash a password with a salt."""
        if salt is None:
            salt = secrets.token_hex(16)
        hashed = hashlib.sha256(f"{salt}{password}".encode()).hexdigest()
        return f"{salt}{hashed}"


class AuthService:
    """Handles user authentication and session management."""
    
    def __init__(self):
        self._users: dict[str, User] = {}
        self._sessions: dict[str, str] = {}
    
    def register(self, username: str, email: str, password: str) -> User:
        """Register a new user."""
        if username in self._users:
            raise ValueError(f"User {username} already exists")
        
        password_hash = User._hash_password(password)
        user = User(username=username, email=email, password_hash=password_hash)
        self._users[username] = user
        return user
    
    def login(self, username: str, password: str) -> str:
        """Authenticate a user and return a session token."""
        user = self._users.get(username)
        if user is None:
            raise ValueError("Invalid credentials")
        
        if not user.verify_password(password):
            raise ValueError("Invalid credentials")
        
        if not user.is_active:
            raise ValueError("Account is disabled")
        
        token = secrets.token_urlsafe(32)
        self._sessions[token] = username
        return token
    
    def logout(self, token: str) -> None:
        """Invalidate a session token."""
        self._sessions.pop(token, None)
    
    def get_current_user(self, token: str) -> Optional[User]:
        """Get the user associated with a session token."""
        username = self._sessions.get(token)
        if username is None:
            return None
        return self._users.get(username)
