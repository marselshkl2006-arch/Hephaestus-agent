use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::tools::ToolResult;

#[derive(Debug, Clone)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Deleted,
}

impl TaskStatus {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub status: TaskStatus,
    pub owner: Option<String>,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
    pub metadata: HashMap<String, Value>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct TaskManager {
    tasks: Arc<Mutex<HashMap<String, Task>>>,
    counter: Arc<Mutex<usize>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            counter: Arc::new(Mutex::new(0)),
        }
    }

    pub async fn create(
        &self,
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Option<HashMap<String, Value>>,
    ) -> Result<String, String> {
        let mut counter = self.counter.lock().await;
        *counter += 1;
        let id = format!("task_{}", counter);

        let meta = metadata.unwrap_or_default();
        // Если metadata не содержит key, можно добавить по умолчанию

        let task = Task {
            id: id.clone(),
            subject,
            description,
            active_form,
            status: TaskStatus::Pending,
            owner: None,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: meta,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };

        let mut tasks = self.tasks.lock().await;
        tasks.insert(id.clone(), task);

        Ok(id)
    }

    pub async fn get(&self, id: &str) -> Option<Task> {
        let tasks = self.tasks.lock().await;
        tasks.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<Task> {
        let tasks = self.tasks.lock().await;
        tasks.values().cloned().collect()
    }

    pub async fn update(
        &self,
        id: &str,
        status: Option<String>,
        subject: Option<String>,
        description: Option<String>,
        active_form: Option<String>,
        owner: Option<String>,
        add_blocks: Option<Vec<String>>,
        add_blocked_by: Option<Vec<String>>,
        metadata: Option<HashMap<String, Value>>,
    ) -> Result<Task, String> {
        let mut tasks = self.tasks.lock().await;
        let task = tasks.get_mut(id).ok_or_else(|| format!("Task {} not found", id))?;

        if let Some(s) = status {
            task.status = TaskStatus::from_str(&s).ok_or_else(|| format!("Invalid status: {}", s))?;
        }
        if let Some(s) = subject {
            task.subject = s;
        }
        if let Some(s) = description {
            task.description = s;
        }
        if let Some(s) = active_form {
            task.active_form = Some(s);
        }
        if let Some(s) = owner {
            task.owner = Some(s);
        }
        if let Some(blocks) = add_blocks {
            task.blocks.extend(blocks);
        }
        if let Some(blocked_by) = add_blocked_by {
            task.blocked_by.extend(blocked_by);
        }
        if let Some(meta) = metadata {
            for (k, v) in meta {
                task.metadata.insert(k, v);
            }
        }
        task.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(task.clone())
    }
}

pub struct TaskCreateTool;
pub struct TaskGetTool;
pub struct TaskListTool;
pub struct TaskUpdateTool;

impl TaskCreateTool {
    pub async fn execute(
        &self,
        manager: &TaskManager,
        subject: String,
        description: String,
        active_form: Option<String>,
        metadata: Option<HashMap<String, Value>>,
    ) -> ToolResult {
        match manager.create(subject, description, active_form, metadata).await {
            Ok(id) => ToolResult {
                success: true,
                output: format!("✅ Task created: {}", id),
                error: None,
                    images: Vec::new(),
            },
            Err(e) => ToolResult {
                success: false,
                output: format!("❌ Failed to create task: {}", e),
                error: Some(e),
                    images: Vec::new(),
            },
        }
    }
}

impl TaskGetTool {
    pub async fn execute(&self, manager: &TaskManager, task_id: String) -> ToolResult {
        match manager.get(&task_id).await {
            Some(task) => {
                let status = task.status.as_str();
                let output = format!(
                    " Task: {}\nSubject: {}\nDescription: {}\nStatus: {}\nOwner: {:?}\nBlocks: {:?}\nBlocked by: {:?}",
                    task.id, task.subject, task.description, status, task.owner, task.blocks, task.blocked_by
                );
                ToolResult {
                    success: true,
                    output,
                    error: None,
                    images: Vec::new(),
                }
            }
            None => ToolResult {
                success: false,
                output: format!("❌ Task {} not found", task_id),
                error: Some("Task not found".to_string()),
                    images: Vec::new(),
            },
        }
    }
}

impl TaskListTool {
    pub async fn execute(&self, manager: &TaskManager) -> ToolResult {
        let tasks = manager.list().await;
        if tasks.is_empty() {
            return ToolResult {
                success: true,
                output: " No tasks found".to_string(),
                error: None,
                    images: Vec::new(),
            };
        }
        let mut lines = vec![format!(" Total tasks: {}", tasks.len())];
        for task in tasks.iter().take(50) {
            lines.push(format!(
                "• {} | {} | {}",
                task.id,
                task.subject,
                task.status.as_str()
            ));
        }
        ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
                images: Vec::new(),
        }
    }
}

impl TaskUpdateTool {
    pub async fn execute(
        &self,
        manager: &TaskManager,
        task_id: String,
        status: Option<String>,
        subject: Option<String>,
        description: Option<String>,
        active_form: Option<String>,
        owner: Option<String>,
        add_blocks: Option<Vec<String>>,
        add_blocked_by: Option<Vec<String>>,
        metadata: Option<HashMap<String, Value>>,
    ) -> ToolResult {
        match manager
            .update(
                &task_id,
                status,
                subject,
                description,
                active_form,
                owner,
                add_blocks,
                add_blocked_by,
                metadata,
            )
            .await
        {
            Ok(task) => ToolResult {
                success: true,
                output: format!("✅ Task {} updated. New status: {}", task.id, task.status.as_str()),
                error: None,
                    images: Vec::new(),
            },
            Err(e) => ToolResult {
                success: false,
                output: format!("❌ Failed to update task: {}", e),
                error: Some(e),
                    images: Vec::new(),
            },
        }
    }
}

pub fn create_task_tools() -> (TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool) {
    (
        TaskCreateTool,
        TaskGetTool,
        TaskListTool,
        TaskUpdateTool,
    )
}
