/// Cron Tools — работа с планировщиком задач

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::tools::ToolResult;

/// Структура задачи
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronTask {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub last_run: Option<String>,
    pub created_at: String,
}

/// Планировщик задач
pub struct Scheduler {
    // ИСПРАВЛЕНО (итерация 5): было `storage_path: Path` — `Path` это
    // unsized-тип (DST), поле такого типа в структуре без `Box`/ссылки
    // не компилируется в принципе. Модуль никогда не собирался ни разу
    // с момента написания (что и объясняет, почему он не был подключён
    // через `mod` — см. STATUS_RU.md, итерации 1-4).
    storage_path: PathBuf,
    tasks: Arc<Mutex<HashMap<String, CronTask>>>,
}

impl Scheduler {
    pub fn new(storage_path: Option<&Path>) -> Self {
        let path = storage_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
                Path::new(&home).join(".hephaestus").join("cron_tasks.json")
            });

        // Создаем директорию, если её нет
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let tasks = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => {
                    match serde_json::from_str::<HashMap<String, CronTask>>(&content) {
                        Ok(map) => map,
                        Err(_) => HashMap::new(),
                    }
                }
                Err(_) => HashMap::new(),
            }
        } else {
            HashMap::new()
        };

        Scheduler {
            storage_path: path,
            tasks: Arc::new(Mutex::new(tasks)),
        }
    }

    async fn save(&self) -> Result<(), String> {
        let tasks = self.tasks.lock().await;
        let json = serde_json::to_string_pretty(&*tasks)
            .map_err(|e| format!("Ошибка сериализации: {}", e))?;
        fs::write(&self.storage_path, json)
            .map_err(|e| format!("Ошибка записи: {}", e))?;
        Ok(())
    }

    pub async fn create(
        &self,
        cron: &str,
        prompt: &str,
        description: Option<&str>,
    ) -> Result<CronTask, String> {
        let mut tasks = self.tasks.lock().await;

        // Проверяем валидность cron выражения (простая проверка)
        let parts: Vec<&str> = cron.split_whitespace().collect();
        if parts.len() != 5 && parts.len() != 6 {
            return Err("Некорректное cron выражение, должно содержать 5 или 6 частей".to_string());
        }

        let id = format!("task_{}_{}", chrono::Utc::now().timestamp(), tasks.len());
        let task = CronTask {
            id: id.clone(),
            cron: cron.to_string(),
            prompt: prompt.to_string(),
            description: description.map(|s| s.to_string()),
            enabled: true,
            last_run: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        tasks.insert(id, task.clone());
        drop(tasks);
        self.save().await?;

        Ok(task)
    }

    pub async fn list_all(&self) -> Vec<CronTask> {
        let tasks = self.tasks.lock().await;
        tasks.values().cloned().collect()
    }

    pub async fn delete(&self, task_id: &str) -> Result<bool, String> {
        let mut tasks = self.tasks.lock().await;
        if tasks.remove(task_id).is_some() {
            drop(tasks);
            self.save().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Создать планировщик задач
pub fn create_scheduler(storage_path: Option<&Path>) -> Scheduler {
    Scheduler::new(storage_path)
}

/// Создать задачу
pub async fn cron_create(
    scheduler: &Scheduler,
    cron: &str,
    prompt: &str,
    description: Option<&str>,
) -> ToolResult {
    match scheduler.create(cron, prompt, description).await {
        Ok(task) => {
            let output = format!(
                "✅ Задача создана: {}\nРасписание: {}\nКоманда: {}\nОписание: {}",
                task.id,
                task.cron,
                task.prompt,
                task.description.unwrap_or_else(|| "Нет".to_string())
            );
            ToolResult::success(output)
        }
        Err(e) => ToolResult::error(e),
    }
}

/// Получить список задач
pub async fn cron_list(scheduler: &Scheduler) -> ToolResult {
    let tasks = scheduler.list_all().await;

    if tasks.is_empty() {
        return ToolResult::success(" Нет запланированных задач".to_string());
    }

    let mut lines = vec![format!(" Запланированных задач: {}\n", tasks.len())];

    for task in tasks {
        let status = if task.enabled { "✅" } else { "❌" };
        lines.push(format!("{} {}", status, task.id));
        lines.push(format!("   Расписание: {}", task.cron));
        lines.push(format!("   Команда: {}", task.prompt));
        if let Some(desc) = task.description {
            lines.push(format!("   Описание: {}", desc));
        }
        if let Some(last_run) = task.last_run {
            lines.push(format!("   Последний запуск: {}", last_run));
        }
        lines.push(String::new());
    }

    ToolResult::success(lines.join("\n"))
}

/// Удалить задачу
pub async fn cron_delete(scheduler: &Scheduler, task_id: &str) -> ToolResult {
    match scheduler.delete(task_id).await {
        Ok(true) => ToolResult::success(format!("✅ Задача {} удалена", task_id)),
        Ok(false) => ToolResult::error(format!("Задача {} не найдена", task_id)),
        Err(e) => ToolResult::error(e),
    }
}

pub fn create_cron_tools(storage_path: Option<&Path>) -> (Scheduler, Vec<(&'static str, Box<dyn Fn()>)>) {
    let scheduler = create_scheduler(storage_path);
    let scheduler_clone = scheduler;
    
    let tools: Vec<(&'static str, Box<dyn Fn()>)> = vec![
        ("cron_create", Box::new(move || {})),
        ("cron_list", Box::new(move || {})),
        ("cron_delete", Box::new(move || {})),
    ];
    
    (scheduler_clone, tools)
}
