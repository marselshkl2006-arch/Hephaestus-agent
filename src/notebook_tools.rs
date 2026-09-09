/// Notebook Tools — работа с Jupyter notebooks

use serde_json::Value;
use std::fs;
use std::path::Path;

use crate::tools::ToolResult;

/// Прочитать Jupyter notebook
pub async fn notebook_read(file_path: &str) -> ToolResult {
    let path = Path::new(file_path);
    if !path.exists() {
        return ToolResult::error(format!("Файл не найден: {}", file_path));
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Ошибка чтения: {}", e)),
    };

    let notebook: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return ToolResult::error(format!("Неверный JSON: {}", e)),
    };

    let empty_vec = vec![];
    let cells = notebook.get("cells").and_then(|c| c.as_array()).unwrap_or(&empty_vec);
    let mut lines = vec![format!(" Notebook: {}\n", path.file_name().unwrap_or_default().to_string_lossy())];
    lines.push(format!("Ячеек: {}\n", cells.len()));

    for (i, cell) in cells.iter().enumerate() {
        let cell_type = cell.get("cell_type").and_then(|v| v.as_str()).unwrap_or("unknown");
        let source = cell
            .get("source")
            .and_then(|s| s.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<String>())
            .unwrap_or_default();

        lines.push(format!("## Ячейка {} ({})", i + 1, cell_type));
        let preview = if source.chars().count() > 200 {
            format!("{}...", crate::truncate_chars(&source, 200))
        } else {
            source
        };
        lines.push(preview);
        lines.push(String::new());
    }

    ToolResult::success(lines.join("\n"))
}

/// Редактировать ячейку в Jupyter notebook
pub async fn notebook_edit(
    file_path: &str,
    cell_index: usize,
    new_content: &str,
) -> ToolResult {
    let path = Path::new(file_path);
    if !path.exists() {
        return ToolResult::error(format!("Файл не найден: {}", file_path));
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Ошибка чтения: {}", e)),
    };

    let mut notebook: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return ToolResult::error(format!("Неверный JSON: {}", e)),
    };

    // ИСПРАВЛЕНО (итерация 5): было `.ok_or_else(...)?` — `?` здесь не
    // компилируется, т.к. функция возвращает `ToolResult`, а не
    // `Result<_, ToolResult>`/`Option<_>` (нет impl `Try` для `ToolResult`).
    // Модуль никогда не собирался — см. cron_tools.rs выше, та же причина
    // (написан без доступа к `cargo build`).
    let cells = match notebook.get_mut("cells").and_then(|c| c.as_array_mut()) {
        Some(cells) => cells,
        None => return ToolResult::error("Не найден массив cells".to_string()),
    };

    if cell_index >= cells.len() {
        return ToolResult::error(format!("Неверный индекс ячейки: {}", cell_index));
    }

    // Обновляем источник
    let cell = &mut cells[cell_index];
    cell["source"] = serde_json::json!(new_content.lines().collect::<Vec<_>>());

    // Сохраняем
    let new_json = match serde_json::to_string_pretty(&notebook) {
        Ok(j) => j,
        Err(e) => return ToolResult::error(format!("Ошибка сериализации: {}", e)),
    };

    if let Err(e) = fs::write(path, new_json) {
        return ToolResult::error(format!("Ошибка записи: {}", e));
    }

    ToolResult::success(format!(
        "✅ Ячейка {} обновлена в {}",
        cell_index,
        path.file_name().unwrap_or_default().to_string_lossy()
    ))
}

/// Создать новый Jupyter notebook
pub async fn notebook_create(file_path: &str) -> ToolResult {
    let path = Path::new(file_path);
    if path.exists() {
        return ToolResult::error(format!("Файл уже существует: {}", file_path));
    }

    let notebook = serde_json::json!({
        "cells": [{
            "cell_type": "code",
            "execution_count": null,
            "metadata": {},
            "outputs": [],
            "source": []
        }],
        "metadata": {
            "kernelspec": {
                "display_name": "Python 3",
                "language": "python",
                "name": "python3"
            },
            "language_info": {
                "name": "python",
                "version": "3.10.0"
            }
        },
        "nbformat": 4,
        "nbformat_minor": 5
    });

    let json_str = match serde_json::to_string_pretty(&notebook) {
        Ok(j) => j,
        Err(e) => return ToolResult::error(format!("Ошибка сериализации: {}", e)),
    };

    if let Err(e) = fs::write(path, json_str) {
        return ToolResult::error(format!("Ошибка записи: {}", e));
    }

    ToolResult::success(format!(
        "✅ Notebook создан: {}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ))
}
