/// Database Tools — работа с SQLite базами данных
/// 
/// Поддерживает:
/// - Создание таблиц
/// - Запросы SELECT, INSERT, UPDATE, DELETE
/// - Выполнение произвольных SQL запросов

use rusqlite::{Connection, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::tools::ToolResult;

/// Менеджер баз данных
pub struct DatabaseManager {
    db_path: String,
    connection: Arc<Mutex<Option<Connection>>>,
}

impl DatabaseManager {
    pub fn new(db_path: &str) -> Self {
        let path = if db_path.is_empty() {
            let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
            format!("{}/.hephaestus/hephaestus.db", home)
        } else {
            db_path.to_string()
        };
        
        // Создаём директорию, если её нет
        if let Some(parent) = Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        Self {
            db_path: path,
            connection: Arc::new(Mutex::new(None)),
        }
    }

    /// Получить соединение с БД
    async fn get_connection(&self) -> Result<Connection, String> {
        let mut conn_guard = self.connection.lock().await;
        
        // Пытаемся взять существующее соединение
        if let Some(conn) = conn_guard.take() {
            // Проверяем, что соединение живо
            match conn.execute("SELECT 1", []) {
                Ok(_) => {
                    // Соединение живое — возвращаем его
                    return Ok(conn);
                }
                Err(_) => {
                    // Соединение умерло — продолжаем и создаём новое
                    // conn упадёт здесь, т.к. мы его забрали через take()
                }
            }
        }

        // Создаём новое соединение
        let conn = Connection::open(&self.db_path)
            .map_err(|e| format!("Ошибка открытия БД: {}", e))?;
        
        // Включаем foreign keys
        conn.execute("PRAGMA foreign_keys = ON", [])
            .map_err(|e| format!("Ошибка включения foreign keys: {}", e))?;

        // Сохраняем соединение обратно в Mutex
        *conn_guard = Some(conn);

        // Забираем его обратно (мы должны вернуть владельца)
        let conn = conn_guard.take().ok_or_else(|| "Не удалось получить соединение".to_string())?;

        Ok(conn)
    }

    /// Выполнить SQL запрос
    pub async fn execute_query(&self, sql: &str) -> ToolResult {
        let conn = match self.get_connection().await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(e),
        };

        // Пытаемся определить тип запроса
        let sql_lower = sql.to_lowercase();
        let trimmed = sql_lower.trim();

        // SELECT запросы
        if trimmed.starts_with("select") || trimmed.starts_with("with") {
            return self.execute_select(&conn, sql).await;
        }

        // INSERT, UPDATE, DELETE, CREATE, DROP, ALTER
        match conn.execute(sql, []) {
            Ok(rows_affected) => {
                let output = if rows_affected > 0 {
                    format!("✅ Запрос выполнен. Затронуто строк: {}", rows_affected)
                } else {
                    "✅ Запрос выполнен успешно.".to_string()
                };
                ToolResult::success(output)
            }
            Err(e) => ToolResult::error(format!("Ошибка выполнения запроса: {}", e)),
        }
    }

    /// Выполнить SELECT запрос и вернуть результаты
    async fn execute_select(&self, conn: &Connection, sql: &str) -> ToolResult {
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Ошибка подготовки запроса: {}", e)),
        };

        let column_names: Vec<String> = stmt
            .column_names()
            .iter()
            .map(|&name| name.to_string())
            .collect();

        let rows = match stmt.query_map([], |row| {
            let mut row_data = Vec::new();
            for i in 0..row.as_ref().column_count() {
                let value: Result<String, rusqlite::Error> = row.get(i);
                match value {
                    Ok(v) => row_data.push(v),
                    Err(_) => row_data.push("NULL".to_string()),
                }
            }
            Ok(row_data)
        }) {
            Ok(rows) => rows,
            Err(e) => return ToolResult::error(format!("Ошибка выполнения запроса: {}", e)),
        };

        let row_data: Vec<Vec<String>> = rows.filter_map(|r| r.ok()).collect();

        if row_data.is_empty() {
            return ToolResult::success("Запрос выполнен. Результатов нет.".to_string());
        }

        // Форматируем вывод в виде таблицы
        let mut output = String::new();
        
        // Заголовки
        let mut widths: Vec<usize> = column_names.iter().map(|name| name.len()).collect();
        for row in &row_data {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }

        // Шапка
        let sep_line: String = widths.iter().map(|&w| "-".repeat(w + 2)).collect::<Vec<_>>().join("+");
        output.push_str(&format!("{}\n", sep_line));
        
        let header: String = column_names.iter().enumerate()
            .map(|(i, name)| format!("{:width$}", name, width = widths[i] + 1))
            .collect::<Vec<_>>()
            .join("|");
        output.push_str(&format!("|{}|\n", header));
        output.push_str(&format!("{}\n", sep_line));

        // Данные
        for row in &row_data {
            let row_str: String = row.iter().enumerate()
                .map(|(i, cell)| format!("{:width$}", cell, width = widths[i] + 1))
                .collect::<Vec<_>>()
                .join("|");
            output.push_str(&format!("|{}|\n", row_str));
        }
        
        output.push_str(&format!("{}\n", sep_line));
        output.push_str(&format!("Всего строк: {}\n", row_data.len()));

        ToolResult::success(output)
    }

    /// Создать таблицу
    pub async fn create_table(&self, table_name: &str, columns: &[(&str, &str)]) -> ToolResult {
        let col_defs: Vec<String> = columns.iter()
            .map(|(name, col_type)| format!("{} {}", name, col_type))
            .collect();
        
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({})",
            table_name,
            col_defs.join(", ")
        );

        self.execute_query(&sql).await
    }

    /// Вставить данные в таблицу
    pub async fn insert(
        &self,
        table_name: &str,
        columns: &[&str],
        values: &[&str],
    ) -> ToolResult {
        if columns.len() != values.len() {
            return ToolResult::error("Количество колонок не совпадает с количеством значений".to_string());
        }

        let placeholders: Vec<String> = (0..values.len()).map(|_| "?".to_string()).collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            columns.join(", "),
            placeholders.join(", ")
        );

        let conn = match self.get_connection().await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(e),
        };

        match conn.execute(&sql, rusqlite::params_from_iter(values.iter())) {
            Ok(rows_affected) => {
                let last_id = conn.last_insert_rowid();
                ToolResult::success(format!(
                    "✅ Вставлено строк: {}, ID: {}",
                    rows_affected, last_id
                ))
            }
            Err(e) => ToolResult::error(format!("Ошибка вставки: {}", e)),
        }
    }

    /// Получить список всех таблиц
    pub async fn list_tables(&self) -> ToolResult {
        self.execute_query(
            "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
        ).await
    }

    /// Получить схему таблицы
    pub async fn table_info(&self, table_name: &str) -> ToolResult {
        let sql = format!("PRAGMA table_info({})", table_name);
        self.execute_query(&sql).await
    }
}

/// Создать менеджер БД
pub fn create_database_manager(db_path: Option<&str>) -> DatabaseManager {
    DatabaseManager::new(db_path.unwrap_or(""))
}

/// Выполнить SQL запрос
pub async fn db_query(db_path: &str, sql: &str) -> ToolResult {
    let db = DatabaseManager::new(db_path);
    db.execute_query(sql).await
}

/// Создать таблицу
pub async fn db_create_table(
    db_path: &str,
    table_name: &str,
    columns: &[(&str, &str)],
) -> ToolResult {
    let db = DatabaseManager::new(db_path);
    db.create_table(table_name, columns).await
}

/// Вставить данные
pub async fn db_insert(
    db_path: &str,
    table_name: &str,
    columns: &[&str],
    values: &[&str],
) -> ToolResult {
    let db = DatabaseManager::new(db_path);
    db.insert(table_name, columns, values).await
}

/// Список таблиц
pub async fn db_list_tables(db_path: &str) -> ToolResult {
    let db = DatabaseManager::new(db_path);
    db.list_tables().await
}

/// Информация о таблице
pub async fn db_table_info(db_path: &str, table_name: &str) -> ToolResult {
    let db = DatabaseManager::new(db_path);
    db.table_info(table_name).await
}
