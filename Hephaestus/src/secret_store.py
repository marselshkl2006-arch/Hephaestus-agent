"""
Secret Store - безопасное хранилище для секретов и API ключей.
Использует шифрование для защиты чувствительных данных.
"""
from __future__ import annotations

import json
import os
from base64 import b64encode, b64decode
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path
from typing import Any

from .real_tools import ToolResult

try:
    from cryptography.fernet import Fernet
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2
    HAS_CRYPTO = True
except ImportError:
    HAS_CRYPTO = False


@dataclass
class Secret:
    """Секрет."""
    key: str
    value: str
    description: str | None
    created_at: str
    updated_at: str
    tags: list[str]


class SecretStore:
    """Безопасное хранилище секретов."""

    def __init__(self, workspace_root: str = "."):
        self.workspace_root = Path(workspace_root)
        self.secrets_dir = Path.home() / ".claude_code" / "secrets"
        self.secrets_dir.mkdir(parents=True, exist_ok=True)
        self.secrets_file = self.secrets_dir / "secrets.enc"
        self.key_file = self.secrets_dir / ".key"

        if not HAS_CRYPTO:
            print("⚠️  Warning: cryptography not installed. Secrets will be stored in plain text!")
            print("   Install with: pip install cryptography")

        self._cipher = self._get_cipher()
        self.secrets: dict[str, Secret] = {}
        self._load_secrets()

    def _get_cipher(self) -> Any:
        """Получить cipher для шифрования."""
        if not HAS_CRYPTO:
            return None

        # Проверяем есть ли ключ
        if self.key_file.exists():
            with open(self.key_file, 'rb') as f:
                key = f.read()
        else:
            # Генерируем новый ключ
            key = Fernet.generate_key()
            with open(self.key_file, 'wb') as f:
                f.write(key)
            # Устанавливаем права только для владельца
            os.chmod(self.key_file, 0o600)

        return Fernet(key)

    def _encrypt(self, data: str) -> str:
        """Зашифровать данные."""
        if not self._cipher:
            # Без шифрования
            return data

        encrypted = self._cipher.encrypt(data.encode())
        return b64encode(encrypted).decode()

    def _decrypt(self, data: str) -> str:
        """Расшифровать данные."""
        if not self._cipher:
            # Без шифрования
            return data

        try:
            encrypted = b64decode(data.encode())
            decrypted = self._cipher.decrypt(encrypted)
            return decrypted.decode()
        except Exception:
            # Возможно данные не зашифрованы
            return data

    def _load_secrets(self) -> None:
        """Загрузить секреты из файла."""
        if not self.secrets_file.exists():
            return

        try:
            with open(self.secrets_file, 'r', encoding='utf-8') as f:
                encrypted_data = f.read()

            if not encrypted_data:
                return

            # Расшифровываем
            decrypted_data = self._decrypt(encrypted_data)
            data = json.loads(decrypted_data)

            # Загружаем секреты
            for key, secret_data in data.items():
                self.secrets[key] = Secret(**secret_data)

        except Exception as e:
            print(f"Warning: Failed to load secrets: {e}")

    def _save_secrets(self) -> None:
        """Сохранить секреты в файл."""
        try:
            # Конвертируем в JSON
            data = {key: asdict(secret) for key, secret in self.secrets.items()}
            json_data = json.dumps(data, indent=2, ensure_ascii=False)

            # Шифруем
            encrypted_data = self._encrypt(json_data)

            # Сохраняем
            with open(self.secrets_file, 'w', encoding='utf-8') as f:
                f.write(encrypted_data)

            # Устанавливаем права только для владельца
            os.chmod(self.secrets_file, 0o600)

        except Exception as e:
            print(f"Error saving secrets: {e}")

    def set_secret(
        self,
        key: str,
        value: str,
        description: str | None = None,
        tags: list[str] | None = None
    ) -> ToolResult:
        """
        Сохранить секрет.

        Args:
            key: Ключ секрета
            value: Значение секрета
            description: Описание
            tags: Теги для категоризации

        Returns:
            ToolResult
        """
        now = datetime.now().isoformat()

        if key in self.secrets:
            # Обновляем существующий
            secret = self.secrets[key]
            secret.value = value
            secret.updated_at = now
            if description:
                secret.description = description
            if tags:
                secret.tags = tags
        else:
            # Создаем новый
            secret = Secret(
                key=key,
                value=value,
                description=description,
                created_at=now,
                updated_at=now,
                tags=tags or []
            )
            self.secrets[key] = secret

        self._save_secrets()

        return ToolResult(
            success=True,
            output=f"Secret saved: {key}",
            error=None
        )

    def get_secret(self, key: str) -> ToolResult:
        """
        Получить секрет.

        Args:
            key: Ключ секрета

        Returns:
            ToolResult со значением секрета
        """
        secret = self.secrets.get(key)
        if not secret:
            return ToolResult(
                success=False,
                output="",
                error=f"Secret not found: {key}"
            )

        return ToolResult(
            success=True,
            output=secret.value,
            error=None
        )

    def list_secrets(self, tag: str | None = None) -> ToolResult:
        """
        Список всех секретов (без значений).

        Args:
            tag: Фильтр по тегу

        Returns:
            ToolResult со списком секретов
        """
        secrets = list(self.secrets.values())

        if tag:
            secrets = [s for s in secrets if tag in s.tags]

        if not secrets:
            return ToolResult(
                success=True,
                output="No secrets found",
                error=None
            )

        output = "Secrets:\n\n"
        for secret in sorted(secrets, key=lambda s: s.key):
            output += f"🔑 {secret.key}\n"
            if secret.description:
                output += f"   {secret.description}\n"
            if secret.tags:
                output += f"   Tags: {', '.join(secret.tags)}\n"
            output += f"   Updated: {secret.updated_at}\n\n"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def delete_secret(self, key: str) -> ToolResult:
        """
        Удалить секрет.

        Args:
            key: Ключ секрета

        Returns:
            ToolResult
        """
        if key not in self.secrets:
            return ToolResult(
                success=False,
                output="",
                error=f"Secret not found: {key}"
            )

        del self.secrets[key]
        self._save_secrets()

        return ToolResult(
            success=True,
            output=f"Secret deleted: {key}",
            error=None
        )

    def export_secrets(self, output_file: str, include_values: bool = False) -> ToolResult:
        """
        Экспортировать секреты в файл.

        Args:
            output_file: Путь к файлу для экспорта
            include_values: Включить значения секретов (опасно!)

        Returns:
            ToolResult
        """
        try:
            data = {}
            for key, secret in self.secrets.items():
                if include_values:
                    data[key] = asdict(secret)
                else:
                    data[key] = {
                        "key": secret.key,
                        "description": secret.description,
                        "tags": secret.tags,
                        "created_at": secret.created_at,
                        "updated_at": secret.updated_at
                    }

            with open(output_file, 'w', encoding='utf-8') as f:
                json.dump(data, f, indent=2, ensure_ascii=False)

            return ToolResult(
                success=True,
                output=f"Secrets exported to: {output_file}",
                error=None
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to export secrets: {e}"
            )

    def import_secrets(self, input_file: str) -> ToolResult:
        """
        Импортировать секреты из файла.

        Args:
            input_file: Путь к файлу для импорта

        Returns:
            ToolResult
        """
        try:
            with open(input_file, 'r', encoding='utf-8') as f:
                data = json.load(f)

            imported = 0
            for key, secret_data in data.items():
                if 'value' in secret_data:
                    # Полный секрет с значением
                    secret = Secret(**secret_data)
                    self.secrets[key] = secret
                    imported += 1

            self._save_secrets()

            return ToolResult(
                success=True,
                output=f"Imported {imported} secrets from: {input_file}",
                error=None
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to import secrets: {e}"
            )

    def rotate_key(self) -> ToolResult:
        """
        Ротация ключа шифрования.

        Returns:
            ToolResult
        """
        if not HAS_CRYPTO:
            return ToolResult(
                success=False,
                output="",
                error="Cryptography not installed"
            )

        try:
            # Сохраняем старые секреты
            old_secrets = dict(self.secrets)

            # Генерируем новый ключ
            new_key = Fernet.generate_key()

            # Сохраняем старый ключ как backup
            backup_file = self.secrets_dir / f".key.backup.{datetime.now().strftime('%Y%m%d_%H%M%S')}"
            if self.key_file.exists():
                with open(self.key_file, 'rb') as f:
                    old_key = f.read()
                with open(backup_file, 'wb') as f:
                    f.write(old_key)

            # Записываем новый ключ
            with open(self.key_file, 'wb') as f:
                f.write(new_key)
            os.chmod(self.key_file, 0o600)

            # Обновляем cipher
            self._cipher = Fernet(new_key)

            # Пересохраняем секреты с новым ключом
            self.secrets = old_secrets
            self._save_secrets()

            return ToolResult(
                success=True,
                output=f"Encryption key rotated. Backup saved to: {backup_file}",
                error=None
            )

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Failed to rotate key: {e}"
            )


# Пример использования
if __name__ == "__main__":
    store = SecretStore()

    # Сохраняем секрет
    result = store.set_secret(
        "github_token",
        "ghp_xxxxxxxxxxxx",
        description="GitHub Personal Access Token",
        tags=["github", "api"]
    )
    print(result.output)

    # Список секретов
    result = store.list_secrets()
    print("\n" + result.output)

    # Получаем секрет
    result = store.get_secret("github_token")
    if result.success:
        print(f"\nSecret value: {result.output}")
