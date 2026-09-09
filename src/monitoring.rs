//! Monitoring System — метрики, алерты, дашборд. Порт `monitoring.py`
//! (итерация 6). Отличие от оригинала: `add_handler` (произвольные
//! Python callbacks по срабатыванию алерта) не портирован — единственный
//! вызывающий код в исходном дереве обработчики не регистрировал
//! (мёртвая фича в самом Python); `check_health()`/`get_dashboard()`
//! возвращают готовый текст, вызывающий код сам решает, куда его вывести.

use serde::Serialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tools::ToolResult;

fn now_ts() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

#[derive(Debug, Clone, Serialize)]
pub struct Metric {
    pub name: String,
    pub value: f64,
    pub timestamp: f64,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub alert_id: String,
    pub name: String,
    pub condition: String,
    pub threshold: f64,
    pub current_value: f64,
    pub triggered_at: f64,
    pub resolved_at: Option<f64>,
    pub severity: String, // info, warning, error, critical
}

#[derive(Debug, Clone, Default)]
struct Aggregate {
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
}
impl Aggregate {
    fn avg(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }
}


pub struct MetricsCollector {
    metrics_file_dir: PathBuf,
    metrics: Mutex<Vec<Metric>>,
    aggregates: Mutex<HashMap<String, Aggregate>>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        let mut dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        dir.push(".hephaestus");
        dir.push("metrics");
        let _ = std::fs::create_dir_all(&dir);
        Self {
            metrics_file_dir: dir,
            metrics: Mutex::new(Vec::new()),
            aggregates: Mutex::new(HashMap::new()),
        }
    }

    pub fn record(&self, name: &str, value: f64, tags: Option<HashMap<String, String>>) {
        let metric = Metric {
            name: name.to_string(),
            value,
            timestamp: now_ts(),
            tags: tags.unwrap_or_default(),
        };

        {
            let mut aggs = self.aggregates.lock().unwrap();
            let agg = aggs.entry(name.to_string()).or_insert_with(|| Aggregate {
                count: 0,
                sum: 0.0,
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
            });
            agg.count += 1;
            agg.sum += value;
            agg.min = agg.min.min(value);
            agg.max = agg.max.max(value);
        }

        let mut metrics = self.metrics.lock().unwrap();
        metrics.push(metric);
        if metrics.len() % 100 == 0 {
            self.flush(&mut metrics);
        }
    }

    fn flush(&self, metrics: &mut Vec<Metric>) {
        if metrics.is_empty() {
            return;
        }
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let path = self.metrics_file_dir.join(format!("metrics_{}.jsonl", date));
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            for m in metrics.iter() {
                if let Ok(line) = serde_json::to_string(m) {
                    let _ = writeln!(f, "{}", line);
                }
            }
        }
        metrics.clear();
    }

    fn get_aggregate(&self, name: &str) -> Option<(u64, f64, f64, f64, f64)> {
        let aggs = self.aggregates.lock().unwrap();
        aggs.get(name).map(|a| (a.count, a.sum, a.min, a.max, a.avg()))
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MonitoringSystem {
    pub metrics: MetricsCollector,
    alerts: Mutex<HashMap<String, Alert>>,
    rules: Vec<(String, String, String, f64, String)>,
}

impl MonitoringSystem {
    pub fn new() -> Self {
        Self {
            metrics: MetricsCollector::new(),
            alerts: Mutex::new(HashMap::new()),
            rules: vec![
                ("High Token Usage".into(), "llm.tokens.total".into(), "gt".into(), 10000.0, "warning".into()),
                ("Slow Tool Execution".into(), "tool.execution.duration_ms".into(), "gt".into(), 5000.0, "warning".into()),
                ("High Error Rate".into(), "tool.execution.error_rate".into(), "gt".into(), 0.1, "error".into()),
            ],
        }
    }

    pub fn record_llm_request(&self, provider: &str, model: &str, tokens: f64, duration_ms: f64) {
        let mut tags = HashMap::new();
        tags.insert("provider".to_string(), provider.to_string());
        tags.insert("model".to_string(), model.to_string());
        self.metrics.record("llm.request.count", 1.0, Some(tags.clone()));
        self.metrics.record("llm.tokens.total", tokens, Some(tags.clone()));
        self.metrics.record("llm.request.duration_ms", duration_ms, Some(tags));
    }

    pub fn record_tool_execution(&self, tool_name: &str, success: bool, duration_ms: f64) {
        let mut tags = HashMap::new();
        tags.insert("tool".to_string(), tool_name.to_string());
        tags.insert("success".to_string(), success.to_string());
        self.metrics.record("tool.execution.count", 1.0, Some(tags));

        let mut tags2 = HashMap::new();
        tags2.insert("tool".to_string(), tool_name.to_string());
        self.metrics.record("tool.execution.duration_ms", duration_ms, Some(tags2.clone()));
        if !success {
            self.metrics.record("tool.execution.errors", 1.0, Some(tags2));
        }
    }

    fn check_rules(&self) -> Vec<Alert> {
        let mut triggered = vec![];
        for (name, metric_name, condition, threshold, severity) in &self.rules {
            let Some((count, _sum, _min, _max, avg)) = self.metrics.get_aggregate(metric_name) else {
                continue;
            };
            if count == 0 {
                continue;
            }
            let should_trigger = match condition.as_str() {
                "gt" => avg > *threshold,
                "lt" => avg < *threshold,
                "eq" => (avg - threshold).abs() < 0.001,
                _ => false,
            };
            if !should_trigger {
                continue;
            }
            let alert_id = format!("{}_{}", name, now_ts() as u64);
            let mut alerts = self.alerts.lock().unwrap();
            if !alerts.contains_key(&alert_id) {
                let alert = Alert {
                    alert_id: alert_id.clone(),
                    name: name.clone(),
                    condition: format!("{} {} {}", metric_name, condition, threshold),
                    threshold: *threshold,
                    current_value: avg,
                    triggered_at: now_ts(),
                    resolved_at: None,
                    severity: severity.clone(),
                };
                alerts.insert(alert_id, alert.clone());
                triggered.push(alert);
            }
        }
        triggered
    }

    pub fn get_active_alerts(&self) -> Vec<Alert> {
        self.alerts.lock().unwrap().values().filter(|a| a.resolved_at.is_none()).cloned().collect()
    }

    pub fn resolve_alert(&self, alert_id: &str) -> bool {
        let mut alerts = self.alerts.lock().unwrap();
        if let Some(a) = alerts.get_mut(alert_id) {
            if a.resolved_at.is_none() {
                a.resolved_at = Some(now_ts());
                return true;
            }
        }
        false
    }

    pub fn get_dashboard(&self) -> ToolResult {
        let mut output = "📊 Monitoring Dashboard\n\n".to_string();

        let llm_requests = self.metrics.get_aggregate("llm.request.count");
        let llm_tokens = self.metrics.get_aggregate("llm.tokens.total");
        let llm_duration = self.metrics.get_aggregate("llm.request.duration_ms");

        output.push_str("## LLM Metrics\n");
        if let Some((count, _, _, _, _)) = llm_requests {
            if count > 0 {
                output.push_str(&format!("- Requests: {}\n", count));
                output.push_str(&format!("- Total Tokens: {:.0}\n", llm_tokens.map(|t| t.1).unwrap_or(0.0)));
                output.push_str(&format!("- Avg Tokens/Request: {:.0}\n", llm_tokens.map(|t| t.4).unwrap_or(0.0)));
                output.push_str(&format!("- Avg Duration: {:.0}ms\n", llm_duration.map(|t| t.4).unwrap_or(0.0)));
            } else {
                output.push_str("- No LLM requests yet\n");
            }
        } else {
            output.push_str("- No LLM requests yet\n");
        }
        output.push('\n');

        let tool_count = self.metrics.get_aggregate("tool.execution.count");
        let tool_duration = self.metrics.get_aggregate("tool.execution.duration_ms");
        let tool_errors = self.metrics.get_aggregate("tool.execution.errors");

        output.push_str("## Tool Metrics\n");
        if let Some((count, _, _, _, _)) = tool_count {
            if count > 0 {
                let errors_sum = tool_errors.map(|t| t.1).unwrap_or(0.0);
                output.push_str(&format!("- Executions: {}\n", count));
                output.push_str(&format!("- Avg Duration: {:.0}ms\n", tool_duration.map(|t| t.4).unwrap_or(0.0)));
                output.push_str(&format!("- Errors: {:.0}\n", errors_sum));
                output.push_str(&format!("- Error Rate: {:.1}%\n", (errors_sum / count as f64) * 100.0));
            } else {
                output.push_str("- No tool executions yet\n");
            }
        } else {
            output.push_str("- No tool executions yet\n");
        }
        output.push('\n');

        let active_alerts = self.get_active_alerts();
        output.push_str(&format!("## Active Alerts ({})\n", active_alerts.len()));
        if active_alerts.is_empty() {
            output.push_str("- No active alerts\n");
        } else {
            for alert in &active_alerts {
                let icon = match alert.severity.as_str() {
                    "info" => "ℹ️",
                    "error" => "❌",
                    "critical" => "🔥",
                    _ => "⚠️",
                };
                output.push_str(&format!("{} {}\n   {}\n   Current: {:.2}, Threshold: {:.2}\n", icon, alert.name, alert.condition, alert.current_value, alert.threshold));
            }
        }

        ToolResult::success(output)
    }

    pub fn check_health(&self) -> ToolResult {
        self.check_rules();
        let active_alerts = self.get_active_alerts();

        let (status, severity) = if active_alerts.is_empty() {
            ("✅ Healthy", "info")
        } else if active_alerts.iter().any(|a| a.severity == "critical") {
            ("🔥 Critical", "critical")
        } else if active_alerts.iter().any(|a| a.severity == "error") {
            ("❌ Error", "error")
        } else {
            ("⚠️  Warning", "warning")
        };

        let output = format!("System Status: {}\n\nActive Alerts: {}\n", status, active_alerts.len());
        ToolResult { success: severity != "critical", output, error: None, images: Vec::new() }
    }
}

impl Default for MonitoringSystem {
    fn default() -> Self {
        Self::new()
    }
}
