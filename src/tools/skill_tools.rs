//! Skill Tools — обёртка над `skill_learner::SkillLearner` в виде обычных
//! инструментов, которые LLM может вызывать сама (раньше `SkillLearner`
//! был объявлен (`mod skill_learner;`) но нигде не создавался и не
//! использовался — мёртвый код).

use std::sync::{Arc, Mutex};
use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolResult};
use crate::skill_learner::SkillLearner;

pub struct SkillListTool {
    learner: Arc<Mutex<SkillLearner>>,
}

impl SkillListTool {
    pub fn new(learner: Arc<Mutex<SkillLearner>>) -> Self {
        Self { learner }
    }
}

#[async_trait]
impl Tool for SkillListTool {
    async fn execute(&self, _args: &Value) -> ToolResult {
        match self.learner.lock() {
            Ok(l) => ToolResult::success(l.list_skills()),
            Err(_) => ToolResult::error("skill_learner mutex poisoned"),
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"query":{"type":"string"},"goal":{"type":"string"},"top_k":{"type":"integer"}}})
    }

    fn name(&self) -> &'static str {
        "skill_list"
    }
}

pub struct SkillFindTool {
    learner: Arc<Mutex<SkillLearner>>,
}

impl SkillFindTool {
    pub fn new(learner: Arc<Mutex<SkillLearner>>) -> Self {
        Self { learner }
    }
}

#[async_trait]
impl Tool for SkillFindTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.is_empty() {
            return ToolResult::error("Требуется параметр 'query'");
        }
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(3) as usize;

        match self.learner.lock() {
            Ok(l) => {
                let found = l.find_similar(query, top_k);
                if found.is_empty() {
                    ToolResult::success("Похожих навыков не найдено".to_string())
                } else {
                    let json = serde_json::to_string_pretty(&found)
                        .unwrap_or_else(|_| "[]".to_string());
                    ToolResult::success(json)
                }
            }
            Err(_) => ToolResult::error("skill_learner mutex poisoned"),
        }
    }
    fn name(&self) -> &'static str {
        "skill_find"
    }
}

pub struct SkillSaveTool {
    learner: Arc<Mutex<SkillLearner>>,
}

impl SkillSaveTool {
    pub fn new(learner: Arc<Mutex<SkillLearner>>) -> Self {
        Self { learner }
    }
}

#[async_trait]
impl Tool for SkillSaveTool {
    async fn execute(&self, args: &Value) -> ToolResult {
        let goal = args.get("goal").and_then(|v| v.as_str()).unwrap_or("");
        if goal.is_empty() {
            return ToolResult::error("Требуется параметр 'goal'");
        }
        let steps: Vec<Value> = args
            .get("steps")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let success_rate = args
            .get("success_rate")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);

        match self.learner.lock() {
            Ok(mut l) => {
                let id = l.learn_from_goal(goal, &steps, success_rate);
                ToolResult::success(format!("Навык сохранён: {}", id))
            }
            Err(_) => ToolResult::error("skill_learner mutex poisoned"),
        }
    }
    fn name(&self) -> &'static str {
        "skill_save"
    }
}
