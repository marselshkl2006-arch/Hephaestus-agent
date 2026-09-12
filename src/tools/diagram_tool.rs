//! Diagram Generator — обёртка над `diagram_tools.rs` (был написан, но
//! нигде не подключён — те же грабли, что с `skill_learner`/`goal_mode`)
//! в виде единого инструмента `diagram` с параметром `kind`.

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolResult};
use crate::diagram_tools::{class_diagram, dependency_graph, file_tree, mermaid_to_ascii, DiagramResult};

fn to_tool_result(r: DiagramResult) -> ToolResult {
    if r.success {
        ToolResult::success(r.output)
    } else {
        ToolResult::error(r.error.unwrap_or_else(|| "Неизвестная ошибка".to_string()))
    }
}

pub struct DiagramTool;

impl DiagramTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for DiagramTool {
    fn description(&self) -> &'static str {
        // ИСПРАВЛЕНО (см. TOOL_AUDIT_REPORT.md: "Ошибка параметров.
        // Требуется kind... Инструмент сырой") — та же причина, что у
        // background_run: не было ни description(), ни
        // parameters_schema(), LLM не могла узнать, что и как передавать.
        "Сгенерировать диаграмму. `kind` определяет, что нужно ещё: \
         class_diagram/dependency_graph — 'file_path'; file_tree — \
         опционально 'directory'+'max_depth'; mermaid_to_ascii — 'mermaid_code'."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["class_diagram", "file_tree", "dependency_graph", "mermaid_to_ascii"],
                    "description": "Тип диаграммы"
                },
                "file_path": {"type": "string", "description": "Нужен для class_diagram и dependency_graph"},
                "directory": {"type": "string", "description": "Для file_tree, по умолчанию текущая директория"},
                "max_depth": {"type": "integer", "description": "Для file_tree, глубина обхода"},
                "mermaid_code": {"type": "string", "description": "Нужен для mermaid_to_ascii — исходный код диаграммы в формате Mermaid"},
                "save_to": {"type": "string", "description": "Путь для сохранения результата в файл (опционально)"}
            },
            "required": ["kind"]
        })
    }

    async fn execute(&self, args: &Value) -> ToolResult {
        let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let save_to = args.get("save_to").and_then(|v| v.as_str());

        // Работа с файловой системой (walkdir/fs) — синхронная, оборачиваем
        // в spawn_blocking, чтобы не блокировать executor на больших деревьях.
        let kind = kind.to_string();
        let save_to = save_to.map(|s| s.to_string());
        let file_path = args.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string());
        let directory = args.get("directory").and_then(|v| v.as_str()).map(|s| s.to_string());
        let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).map(|v| v as usize);
        let mermaid_code = args.get("mermaid_code").and_then(|v| v.as_str()).map(|s| s.to_string());

        let result = tokio::task::spawn_blocking(move || match kind.as_str() {
            "class_diagram" => {
                let Some(fp) = file_path else { return DiagramResult::err("Требуется 'file_path'") };
                class_diagram(&fp, save_to.as_deref())
            }
            "file_tree" => file_tree(directory.as_deref(), max_depth, save_to.as_deref()),
            "dependency_graph" => {
                let Some(fp) = file_path else { return DiagramResult::err("Требуется 'file_path'") };
                dependency_graph(&fp, save_to.as_deref())
            }
            "mermaid_to_ascii" => {
                let Some(code) = mermaid_code else { return DiagramResult::err("Требуется 'mermaid_code'") };
                mermaid_to_ascii(&code, save_to.as_deref())
            }
            other => DiagramResult::err(format!(
                "Неизвестный kind: '{}'. Доступно: class_diagram, file_tree, dependency_graph, mermaid_to_ascii",
                other
            )),
        })
        .await;

        match result {
            Ok(r) => to_tool_result(r),
            Err(e) => ToolResult::error(format!("Ошибка выполнения: {}", e)),
        }
    }

    fn name(&self) -> &'static str {
        "diagram"
    }
}
