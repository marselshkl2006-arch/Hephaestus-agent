/// OCR Tool — распознавание текста с изображений через Tesseract

use std::path::Path;
use std::process::Command;
use std::fs;
use tempfile::NamedTempFile;

use crate::tools::ToolResult;

const SUPPORTED: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".bmp", ".tiff", ".tif", ".gif", ".webp",
];

/// Проверить наличие tesseract
pub fn check_tesseract() -> bool {
    let output = Command::new("tesseract")
        .arg("--version")
        .output();
    output.is_ok() && output.unwrap().status.success()
}

/// Извлечь текст из изображения через tesseract
pub async fn ocr_extract(image_path: &str, lang: &str, psm: i32) -> ToolResult {
    let path = Path::new(image_path);
    if !path.exists() {
        return ToolResult::error(format!("Файл не найден: {}", image_path));
    }

    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();

    if !SUPPORTED.contains(&ext.as_str()) {
        return ToolResult::error(format!(
            "Неподдерживаемый формат: {}. Поддерживаются: {}",
            ext,
            SUPPORTED.join(", ")
        ));
    }

    if !check_tesseract() {
        return ToolResult::error(
            "Tesseract не установлен. Установите: sudo apt install tesseract-ocr tesseract-ocr-rus".to_string()
        );
    }

    // Создаем временный файл для вывода
    let temp_file = match NamedTempFile::new() {
        Ok(f) => f,
        Err(e) => return ToolResult::error(format!("Ошибка создания временного файла: {}", e)),
    };
    let out_base = temp_file.path().to_str().unwrap_or("temp").to_string();
    // Закрываем файл, чтобы tesseract мог записать в него
    drop(temp_file);

    let cmd = Command::new("tesseract")
        .arg(image_path)
        .arg(&out_base)
        .arg("-l")
        .arg(lang)
        .arg("--psm")
        .arg(psm.to_string())
        .output();

    let output = match cmd {
        Ok(o) => o,
        Err(e) => return ToolResult::error(format!("Ошибка запуска tesseract: {}", e)),
    };

    // Читаем результат
    let out_path = format!("{}.txt", out_base);
    let out_file = Path::new(&out_path);
    if !out_file.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return ToolResult::error(format!("Tesseract не создал выходной файл: {}", stderr));
    }

    let text = match fs::read_to_string(out_file) {
        Ok(t) => t.trim().to_string(),
        Err(e) => return ToolResult::error(format!("Ошибка чтения результата: {}", e)),
    };

    // Удаляем временный файл
    let _ = fs::remove_file(out_file);

    if text.is_empty() {
        return ToolResult::success("Текст не найден на изображении".to_string());
    }

    let lines = text.lines().count();
    let chars = text.chars().count();
    let output_text = format!(
        "✅ Распознан текст ({} строк, {} символов):\n\n{}",
        lines, chars, text
    );
    ToolResult::success(output_text)
}

/// Получить список доступных языков Tesseract
pub async fn ocr_languages() -> ToolResult {
    if !check_tesseract() {
        return ToolResult::error("Tesseract не установлен".to_string());
    }

    let output = Command::new("tesseract")
        .arg("--list-langs")
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            let result = format!("{}{}", stdout, stderr);
            ToolResult::success(result)
        }
        Err(e) => ToolResult::error(format!("Ошибка: {}", e)),
    }
}

/// Проверить статус OCR
pub async fn ocr_status() -> ToolResult {
    let mut lines = Vec::new();

    if check_tesseract() {
        let version = Command::new("tesseract")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                s.lines().next().map(|l| l.to_string())
            })
            .unwrap_or_else(|| "ok".to_string());
        lines.push(format!("✅ Tesseract: {}", version));
    } else {
        lines.push("❌ Tesseract: не установлен (sudo apt install tesseract-ocr tesseract-ocr-rus)".to_string());
    }

    let has_tesseract = check_tesseract();
    ToolResult {
        success: has_tesseract,
        output: lines.join("\n"),
        error: if has_tesseract { None } else { Some("Tesseract не установлен".to_string()) },
            images: Vec::new(),
        }
}
