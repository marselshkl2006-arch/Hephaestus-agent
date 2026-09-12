//! Goal Mode — автономное выполнение целей.
//!
//! Режим, в котором агент получает высокоуровневую цель и самостоятельно
//! выполняет её, используя инструменты, пока не достигнет цели или
//! не исчерпает бюджет итераций.

use crate::Agent;
use serde_json::Value;
use std::path::PathBuf;

/// Шаг выполнения цели — реальный вызов инструмента.
#[derive(Debug, Clone)]
pub struct GoalStep {
    pub step_num: usize,
    pub description: String,
    pub tool: String,
    pub params: Value,
    pub status: GoalStepStatus,
    pub result: String,
}

/// Статус шага.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStepStatus {
    Pending,
    Done,
    Failed,
}

/// Цель — описание и список выполненных шагов.
#[derive(Debug, Clone)]
pub struct Goal {
    pub description: String,
    pub steps: Vec<GoalStep>,
    pub status: GoalStatus,
    pub report: String,
}

/// Статус цели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Planning,
    Executing,
    Done,
    Failed,
}

/// Агент в режиме цели.
pub struct GoalModeAgent<'a> {
    agent: &'a mut Agent,
    on_progress: Box<dyn Fn(&str) + Send + Sync>,
    current_goal: Option<Goal>,
}

impl<'a> GoalModeAgent<'a> {
    /// Создать новый GoalModeAgent.
    pub fn new(agent: &'a mut Agent) -> Self {
        Self {
            agent,
            // ИСПРАВЛЕНО: было println!() — тот же класс бага, что
            // eprintln! в Agent::chat() (main.rs, см. комментарий там):
            // в TUI-режиме прямая печать в stdout из фонового таска рвёт
            // рамки ratatui. Дефолт теперь пишет в файловый лог; REPL сам
            // подписывается через on_progress(), чтобы прогресс всё равно
            // доходил до пользователя — но безопасным путём (см. repl.rs).
            on_progress: Box::new(|msg| crate::logging_system::info(msg)),
            current_goal: None,
        }
    }

    /// Установить функцию обратного вызова для прогресса.
    pub fn on_progress<F>(&mut self, callback: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_progress = Box::new(callback);
    }

    /// Выполнить цель.
    pub async fn pursue(&mut self, goal_text: &str, save_report_to: Option<&str>) -> String {
        (self.on_progress)(&format!("\n Цель: {}", goal_text));

        let mut goal = Goal {
            description: goal_text.to_string(),
            steps: Vec::new(),
            status: GoalStatus::Executing,
            report: String::new(),
        };

        self.current_goal = Some(goal.clone());

        let goal_framing = format!(
            "{}\n\n{}",
            goal_text,
            "ЗАДАЧА ВЫШЕ — это ЦЕЛЬ, а не разовый запрос.\n\n\
             Работай самостоятельно, шаг за шагом, вызывая столько инструментов\n\
             подряд, сколько нужно. Если для цели нужно сначала что-то изучить\n\
             (структуру проекта, существующий код, репозиторий) — сделай это\n\
             ПЕРЕД тем как писать/создавать что-либо, и используй увиденное.\n\
             Если какой-то шаг не удался — не сдавайся, попробуй скорректировать\n\
             подход и продолжай.\n\n\
             Останавливайся и пиши финальный текстовый ответ ТОЛЬКО когда цель\n\
             полностью выполнена (или ты уверен, что дальше двигаться нельзя).\n\
             Не проси подтверждения — действуй."
        );

        // Отслеживаем вызовы инструментов через канал: замыкание-хук должно
        // быть 'static (Agent хранит его как Box<dyn Fn + Send + Sync>, без
        // лайфтайма), поэтому вместо заимствования `&mut goal` изнутри хука
        // события просто отправляются в канал и разбираются здесь после
        // того как `chat()` полностью завершится.
        struct ToolEvent {
            name: String,
            params: Value,
            success: bool,
            output: String,
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ToolEvent>();

        let hook_tx = tx.clone();
        let hook = Box::new(move |name: &str, params: &Value, success: bool, output: &str| {
            let _ = hook_tx.send(ToolEvent {
                name: name.to_string(),
                params: params.clone(),
                success,
                output: output.to_string(),
            });
        });
        self.agent.set_tool_hook(Some(hook));

        let final_text = self.agent.chat(&goal_framing, 80, true).await;

        self.agent.set_tool_hook(None);
        drop(tx);

        while let Ok(event) = rx.try_recv() {
            let step_num = goal.steps.len() + 1;
            let status = if event.success {
                GoalStepStatus::Done
            } else {
                GoalStepStatus::Failed
            };
            let description = format!(
                "{}({})",
                event.name,
                event
                    .params
                    .as_object()
                    .map(|obj| {
                        obj.iter()
                            .take(3)
                            .map(|(k, v)| format!("{}={:?}", k, v))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            );
            let icon = if event.success { "✅" } else { "❌" };
            let truncated = if event.output.chars().count() > 100 {
                format!("{}…", crate::truncate_chars(&event.output, 100))
            } else {
                event.output.clone()
            };
            (self.on_progress)(&format!("⚒️  Шаг {}: {} — {} {}", step_num, event.name, icon, truncated));

            goal.steps.push(GoalStep {
                step_num,
                description,
                tool: event.name,
                params: event.params,
                status,
                result: event.output,
            });
        }

        let done = goal.steps.iter().filter(|s| s.status == GoalStepStatus::Done).count();
        let failed = goal.steps.iter().filter(|s| s.status == GoalStepStatus::Failed).count();
        goal.status = if failed == 0 {
            GoalStatus::Done
        } else if done == 0 {
            GoalStatus::Failed
        } else {
            GoalStatus::Done
        };

        let mut report_lines = Vec::new();
        report_lines.push(format!("## Отчёт по цели: {}\n", goal_text));
        report_lines.push(format!("✅ Выполнено шагов: {}", done));
        report_lines.push(format!("❌ Ошибок: {}\n", failed));

        if !goal.steps.is_empty() {
            report_lines.push("### Шаги:".to_string());
            for s in &goal.steps {
                let icon = if s.status == GoalStepStatus::Done { "✅" } else { "❌" };
                report_lines.push(format!("{} {}. {}", icon, s.step_num, s.description));
                if !s.result.is_empty() {
                    let truncated = if s.result.chars().count() > 150 {
                        format!("{}…", crate::truncate_chars(&s.result, 150))
                    } else {
                        s.result.clone()
                    };
                    report_lines.push(format!("   Результат: {}", truncated));
                }
            }
            report_lines.push(String::new());
        }

        report_lines.push("### Итог модели:".to_string());
        report_lines.push(final_text.clone());

        let report = report_lines.join("\n");
        goal.report = report.clone();
        self.current_goal = Some(goal);

        if let Some(path) = save_report_to {
            if let Some(parent) = PathBuf::from(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, &report);
            (self.on_progress)(&format!("\n Отчёт сохранён: {}", path));
        }

        (self.on_progress)(&format!("\n Цель завершена: {} шагов выполнено, {} ошибок", done, failed));
        report
    }
}
