"""
Database Tools - инструменты для работы с базами данных.
Поддержка: SQLite, PostgreSQL, MySQL.
"""
from __future__ import annotations

import json
import sqlite3
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass
class ToolResult:
    """Результат выполнения инструмента."""
    success: bool
    output: str
    error: str = ""


class DatabaseQueryTool:
    """Выполнение SQL запросов к базам данных."""

    def __init__(self):
        self.connections = {}

    def query(
        self,
        database: str,
        query: str,
        params: list[Any] | None = None,
        db_type: str = "sqlite",
        host: str | None = None,
        port: int | None = None,
        user: str | None = None,
        password: str | None = None
    ) -> ToolResult:
        """Выполнить SQL запрос.

        Args:
            database: Путь к БД (SQLite) или имя БД (PostgreSQL/MySQL)
            query: SQL запрос
            params: Параметры для запроса (для безопасности)
            db_type: Тип БД (sqlite, postgresql, mysql)
            host: Хост БД (для PostgreSQL/MySQL)
            port: Порт БД
            user: Пользователь БД
            password: Пароль БД

        Returns:
            ToolResult с результатами запроса
        """
        if db_type == "sqlite":
            return self._query_sqlite(database, query, params)
        elif db_type == "postgresql":
            return self._query_postgresql(database, query, params, host, port, user, password)
        elif db_type == "mysql":
            return self._query_mysql(database, query, params, host, port, user, password)
        else:
            return ToolResult(
                success=False,
                output="",
                error=f"Unsupported database type: {db_type}. Use: sqlite, postgresql, mysql"
            )

    def _query_sqlite(self, database: str, query: str, params: list[Any] | None) -> ToolResult:
        """Запрос к SQLite."""
        try:
            db_path = Path(database)
            if not db_path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Database not found: {database}"
                )

            conn = sqlite3.connect(str(db_path))
            cursor = conn.cursor()

            # Выполняем запрос
            if params:
                cursor.execute(query, params)
            else:
                cursor.execute(query)

            # Если SELECT - получаем результаты
            if query.strip().upper().startswith('SELECT'):
                rows = cursor.fetchall()
                columns = [desc[0] for desc in cursor.description] if cursor.description else []

                output = self._format_results(columns, rows)
            else:
                # Для INSERT/UPDATE/DELETE - коммитим и показываем количество изменённых строк
                conn.commit()
                output = f"Query executed successfully.\nRows affected: {cursor.rowcount}"

            conn.close()

            return ToolResult(success=True, output=output)

        except sqlite3.Error as e:
            return ToolResult(
                success=False,
                output="",
                error=f"SQLite error: {str(e)}"
            )
        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def _query_postgresql(
        self,
        database: str,
        query: str,
        params: list[Any] | None,
        host: str | None,
        port: int | None,
        user: str | None,
        password: str | None
    ) -> ToolResult:
        """Запрос к PostgreSQL."""
        try:
            import psycopg2
        except ImportError:
            return ToolResult(
                success=False,
                output="",
                error="psycopg2 not installed. Install: pip install psycopg2-binary"
            )

        try:
            conn = psycopg2.connect(
                database=database,
                host=host or 'localhost',
                port=port or 5432,
                user=user,
                password=password
            )
            cursor = conn.cursor()

            # Выполняем запрос
            if params:
                cursor.execute(query, params)
            else:
                cursor.execute(query)

            # Если SELECT - получаем результаты
            if query.strip().upper().startswith('SELECT'):
                rows = cursor.fetchall()
                columns = [desc[0] for desc in cursor.description] if cursor.description else []

                output = self._format_results(columns, rows)
            else:
                conn.commit()
                output = f"Query executed successfully.\nRows affected: {cursor.rowcount}"

            conn.close()

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"PostgreSQL error: {str(e)}"
            )

    def _query_mysql(
        self,
        database: str,
        query: str,
        params: list[Any] | None,
        host: str | None,
        port: int | None,
        user: str | None,
        password: str | None
    ) -> ToolResult:
        """Запрос к MySQL."""
        try:
            import mysql.connector
        except ImportError:
            return ToolResult(
                success=False,
                output="",
                error="mysql-connector not installed. Install: pip install mysql-connector-python"
            )

        try:
            conn = mysql.connector.connect(
                database=database,
                host=host or 'localhost',
                port=port or 3306,
                user=user,
                password=password
            )
            cursor = conn.cursor()

            # Выполняем запрос
            if params:
                cursor.execute(query, params)
            else:
                cursor.execute(query)

            # Если SELECT - получаем результаты
            if query.strip().upper().startswith('SELECT'):
                rows = cursor.fetchall()
                columns = [desc[0] for desc in cursor.description] if cursor.description else []

                output = self._format_results(columns, rows)
            else:
                conn.commit()
                output = f"Query executed successfully.\nRows affected: {cursor.rowcount}"

            conn.close()

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"MySQL error: {str(e)}"
            )

    def _format_results(self, columns: list[str], rows: list[tuple]) -> str:
        """Форматировать результаты запроса."""
        if not rows:
            return "No results found."

        # Вычисляем ширину колонок
        col_widths = [len(col) for col in columns]
        for row in rows:
            for i, val in enumerate(row):
                col_widths[i] = max(col_widths[i], len(str(val)))

        # Форматируем заголовок
        output = ""
        header = " | ".join(col.ljust(col_widths[i]) for i, col in enumerate(columns))
        output += header + "\n"
        output += "-" * len(header) + "\n"

        # Форматируем строки
        for row in rows[:100]:  # Ограничиваем 100 строками
            output += " | ".join(str(val).ljust(col_widths[i]) for i, val in enumerate(row)) + "\n"

        if len(rows) > 100:
            output += f"\n... and {len(rows) - 100} more rows"

        output += f"\n\nTotal rows: {len(rows)}"

        return output

    def get_schema(
        self,
        database: str,
        db_type: str = "sqlite",
        host: str | None = None,
        port: int | None = None,
        user: str | None = None,
        password: str | None = None
    ) -> ToolResult:
        """Получить схему базы данных.

        Args:
            database: Путь к БД или имя БД
            db_type: Тип БД (sqlite, postgresql, mysql)
            host: Хост БД
            port: Порт БД
            user: Пользователь БД
            password: Пароль БД

        Returns:
            ToolResult со схемой БД
        """
        if db_type == "sqlite":
            return self._get_schema_sqlite(database)
        elif db_type == "postgresql":
            return self._get_schema_postgresql(database, host, port, user, password)
        elif db_type == "mysql":
            return self._get_schema_mysql(database, host, port, user, password)
        else:
            return ToolResult(
                success=False,
                output="",
                error=f"Unsupported database type: {db_type}"
            )

    def _get_schema_sqlite(self, database: str) -> ToolResult:
        """Получить схему SQLite."""
        try:
            db_path = Path(database)
            if not db_path.exists():
                return ToolResult(
                    success=False,
                    output="",
                    error=f"Database not found: {database}"
                )

            conn = sqlite3.connect(str(db_path))
            cursor = conn.cursor()

            # Получаем список таблиц
            cursor.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            tables = cursor.fetchall()

            output = f"=== Database Schema: {database} ===\n\n"
            output += f"Tables: {len(tables)}\n\n"

            # Для каждой таблицы получаем структуру
            for (table_name,) in tables:
                output += f"Table: {table_name}\n"
                cursor.execute(f"PRAGMA table_info({table_name})")
                columns = cursor.fetchall()

                for col in columns:
                    col_id, name, col_type, not_null, default, pk = col
                    output += f"  - {name} ({col_type})"
                    if pk:
                        output += " PRIMARY KEY"
                    if not_null:
                        output += " NOT NULL"
                    if default:
                        output += f" DEFAULT {default}"
                    output += "\n"

                output += "\n"

            conn.close()

            return ToolResult(success=True, output=output)

        except Exception as e:
            return ToolResult(
                success=False,
                output="",
                error=f"Error: {str(e)}"
            )

    def _get_schema_postgresql(
        self,
        database: str,
        host: str | None,
        port: int | None,
        user: str | None,
        password: str | None
    ) -> ToolResult:
        """Получить схему PostgreSQL."""
        query = """
        SELECT table_name 
        FROM information_schema.tables 
        WHERE table_schema = 'public'
        ORDER BY table_name
        """
        
        result = self._query_postgresql(database, query, None, host, port, user, password)
        if not result.success:
            return result

        return ToolResult(
            success=True,
            output=f"=== Database Schema: {database} ===\n\n{result.output}"
        )

    def _get_schema_mysql(
        self,
        database: str,
        host: str | None,
        port: int | None,
        user: str | None,
        password: str | None
    ) -> ToolResult:
        """Получить схему MySQL."""
        query = "SHOW TABLES"
        
        result = self._query_mysql(database, query, None, host, port, user, password)
        if not result.success:
            return result

        return ToolResult(
            success=True,
            output=f"=== Database Schema: {database} ===\n\n{result.output}"
        )
