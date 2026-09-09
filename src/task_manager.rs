use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};

/// Статус задачи
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TaskStatus::Pending => "⏳ Ожидание",
            TaskStatus::InProgress => " Выполняется",
            TaskStatus::Completed => "✅ Завершена",
            TaskStatus::Failed => "❌ Ошибка",
            TaskStatus::Cancelled => " Отменена",
        };
        write!(f, "{}", s)
    }
}

/// Приоритет задачи
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TaskPriority::Low => " Низкий",
            TaskPriority::Medium => " Средний",
            TaskPriority::High => " Высокий",
            TaskPriority::Critical => " Критический",
        };
        write!(f, "{}", s)
    }
}

/// Структура задачи
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub created_at: u64,
    pub updated_at: u64,
    pub due_date: Option<u64>,
    pub assigned_to: Option<String>,
    pub tags: Vec<String>,
    pub subtasks: Vec<Task>,
    pub metadata: HashMap<String, String>,
}

impl Task {
    pub fn new(title: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        Self {
            id: format!("task_{}", now),
            title: title.to_string(),
            description: None,
            status: TaskStatus::Pending,
            priority: TaskPriority::Medium,
            created_at: now,
            updated_at: now,
            due_date: None,
            assigned_to: None,
            tags: Vec::new(),
            subtasks: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_due_date(mut self, timestamp: u64) -> Self {
        self.due_date = Some(timestamp);
        self
    }

    pub fn with_assigned_to(mut self, assignee: &str) -> Self {
        self.assigned_to = Some(assignee.to_string());
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn add_tag(&mut self, tag: &str) {
        if !self.tags.contains(&tag.to_string()) {
            self.tags.push(tag.to_string());
        }
    }

    pub fn add_subtask(&mut self, subtask: Task) {
        self.subtasks.push(subtask);
    }

    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        self.updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }

    pub fn is_completed(&self) -> bool {
        matches!(self.status, TaskStatus::Completed)
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status, TaskStatus::Pending | TaskStatus::InProgress)
    }

    pub fn progress(&self) -> f32 {
        if self.subtasks.is_empty() {
            return if self.is_completed() { 1.0 } else { 0.0 };
        }
        let completed = self.subtasks.iter().filter(|t| t.is_completed()).count();
        completed as f32 / self.subtasks.len() as f32
    }

    pub fn format(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!(" {}\n", self.title));
        output.push_str(&format!("   ID: {}\n", self.id));
        output.push_str(&format!("   Статус: {}\n", self.status));
        output.push_str(&format!("   Приоритет: {}\n", self.priority));
        output.push_str(&format!("   Прогресс: {:.0}%\n", self.progress() * 100.0));
        
        if let Some(desc) = &self.description {
            output.push_str(&format!("   Описание: {}\n", desc));
        }
        
        if let Some(due) = self.due_date {
            output.push_str(&format!("   Срок: {}\n", due));
        }
        
        if let Some(assignee) = &self.assigned_to {
            output.push_str(&format!("   Назначена: {}\n", assignee));
        }
        
        if !self.tags.is_empty() {
            output.push_str(&format!("   Теги: {}\n", self.tags.join(", ")));
        }
        
        if !self.subtasks.is_empty() {
            output.push_str("   Подзадачи:\n");
            for subtask in &self.subtasks {
                let done = if subtask.is_completed() { "✅" } else { "⬜" };
                output.push_str(&format!("     {} {} [{}]\n", done, subtask.title, subtask.status));
            }
        }
        
        output
    }
}

/// Менеджер задач
pub struct TaskManager {
    tasks: HashMap<String, Task>,
    current_id: u64,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            current_id: 0,
        }
    }

    /// Создать новую задачу
    pub fn create_task(&mut self, title: &str) -> String {
        let mut task = Task::new(title);
        // Убедимся, что ID уникальный
        while self.tasks.contains_key(&task.id) {
            self.current_id += 1;
            task.id = format!("task_{}", self.current_id);
        }
        let id = task.id.clone();
        self.tasks.insert(id.clone(), task);
        id
    }

    /// Создать задачу с полными параметрами
    pub fn create_task_full(&mut self, mut task: Task) -> String {
        while self.tasks.contains_key(&task.id) {
            self.current_id += 1;
            task.id = format!("task_{}", self.current_id);
        }
        let id = task.id.clone();
        self.tasks.insert(id.clone(), task);
        id
    }

    /// Получить задачу по ID
    pub fn get_task(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// Получить задачу по ID (мутабельно)
    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(id)
    }

    /// Обновить статус задачи
    pub fn update_status(&mut self, id: &str, status: TaskStatus) -> bool {
        if let Some(task) = self.tasks.get_mut(id) {
            task.set_status(status);
            true
        } else {
            false
        }
    }

    /// Добавить подзадачу
    pub fn add_subtask(&mut self, parent_id: &str, mut subtask: Task) -> bool {
        // Проверяем, существует ли родительская задача
        if !self.tasks.contains_key(parent_id) {
            return false;
        }
        
        // Генерируем уникальный ID для подзадачи
        while self.tasks.contains_key(&subtask.id) {
            self.current_id += 1;
            subtask.id = format!("task_{}", self.current_id);
        }
        
        // Теперь мутабельно заимствуем родителя
        if let Some(parent) = self.tasks.get_mut(parent_id) {
            parent.add_subtask(subtask);
            true
        } else {
            false
        }
    }

    /// Удалить задачу
    pub fn delete_task(&mut self, id: &str) -> bool {
        self.tasks.remove(id).is_some()
    }

    /// Получить все задачи
    pub fn get_all_tasks(&self) -> Vec<&Task> {
        self.tasks.values().collect()
    }

    /// Получить задачи по статусу
    pub fn get_tasks_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.status == status)
            .collect()
    }

    /// Получить задачи по приоритету
    pub fn get_tasks_by_priority(&self, priority: TaskPriority) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.priority == priority)
            .collect()
    }

    /// Получить задачи по тегу
    pub fn get_tasks_by_tag(&self, tag: &str) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.tags.contains(&tag.to_string()))
            .collect()
    }

    /// Получить задачи, назначенные пользователю
    pub fn get_tasks_assigned_to(&self, assignee: &str) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.assigned_to.as_deref() == Some(assignee))
            .collect()
    }

    /// Получить активные задачи
    pub fn get_active_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.is_active())
            .collect()
    }

    /// Получить завершённые задачи
    pub fn get_completed_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.is_completed())
            .collect()
    }

    /// Поиск задач по заголовку или описанию
    pub fn search_tasks(&self, query: &str) -> Vec<&Task> {
        let query_lower = query.to_lowercase();
        self.tasks
            .values()
            .filter(|t| {
                t.title.to_lowercase().contains(&query_lower)
                    || t.description
                        .as_ref()
                        .map(|d| d.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
            })
            .collect()
    }

    /// Получить статистику
    pub fn get_stats(&self) -> TaskStats {
        let total = self.tasks.len();
        let completed = self.tasks.values().filter(|t| t.is_completed()).count();
        let active = self.tasks.values().filter(|t| t.is_active()).count();
        let pending = self.tasks.values().filter(|t| t.status == TaskStatus::Pending).count();
        let in_progress = self.tasks.values().filter(|t| t.status == TaskStatus::InProgress).count();
        let failed = self.tasks.values().filter(|t| t.status == TaskStatus::Failed).count();
        let cancelled = self.tasks.values().filter(|t| t.status == TaskStatus::Cancelled).count();
        
        TaskStats {
            total,
            completed,
            active,
            pending,
            in_progress,
            failed,
            cancelled,
        }
    }

    /// Экспорт в JSON
    pub fn export_json(&self) -> String {
        let tasks_vec: Vec<&Task> = self.tasks.values().collect();
        serde_json::to_string_pretty(&tasks_vec).unwrap_or_default()
    }

    /// Импорт из JSON
    pub fn import_json(&mut self, json: &str) -> Result<usize, String> {
        let tasks: Vec<Task> = serde_json::from_str(json)
            .map_err(|e| format!("Ошибка парсинга JSON: {}", e))?;
        
        let count = tasks.len();
        for task in tasks {
            self.create_task_full(task);
        }
        Ok(count)
    }

    /// Очистить все задачи
    pub fn clear(&mut self) {
        self.tasks.clear();
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Статистика задач
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStats {
    pub total: usize,
    pub completed: usize,
    pub active: usize,
    pub pending: usize,
    pub in_progress: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl TaskStats {
    pub fn format(&self) -> String {
        let mut output = String::new();
        output.push_str(" Статистика задач\n");
        output.push_str(&format!("   Всего: {}\n", self.total));
        output.push_str(&format!("   ✅ Завершено: {}\n", self.completed));
        output.push_str(&format!("    Выполняется: {}\n", self.in_progress));
        output.push_str(&format!("   ⏳ Ожидает: {}\n", self.pending));
        output.push_str(&format!("   ❌ Ошибок: {}\n", self.failed));
        output.push_str(&format!("    Отменено: {}\n", self.cancelled));
        
        let progress = if self.total > 0 {
            (self.completed as f32 / self.total as f32) * 100.0
        } else {
            0.0
        };
        output.push_str(&format!("    Прогресс: {:.1}%\n", progress));
        
        output
    }
}

/// Создать менеджер задач
pub fn create_task_manager() -> TaskManager {
    TaskManager::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_new() {
        let task = Task::new("Test task");
        assert_eq!(task.title, "Test task");
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.priority, TaskPriority::Medium);
    }

    #[test]
    fn test_task_manager_create() {
        let mut manager = TaskManager::new();
        let id = manager.create_task("Task 1");
        assert!(manager.get_task(&id).is_some());
    }

    #[test]
    fn test_task_manager_update_status() {
        let mut manager = TaskManager::new();
        let id = manager.create_task("Task 1");
        assert!(manager.update_status(&id, TaskStatus::Completed));
        let task = manager.get_task(&id).unwrap();
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[test]
    fn test_task_manager_stats() {
        let mut manager = TaskManager::new();
        let id1 = manager.create_task("Task 1");
        let id2 = manager.create_task("Task 2");
        manager.update_status(&id1, TaskStatus::Completed);
        
        let stats = manager.get_stats();
        assert_eq!(stats.total, 2);
        assert_eq!(stats.completed, 1);
    }
}
