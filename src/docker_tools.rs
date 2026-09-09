/// Docker Tools — управление Docker контейнерами

use std::process::Command;
use serde_json::Value;

use crate::tools::ToolResult;

const TIMEOUT_SECONDS: u64 = 30;

/// Выполнить docker команду с проверкой
fn run_docker(args: &[&str]) -> Result<String, String> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| format!("Docker not found: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Docker error: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Список контейнеров
pub async fn docker_list(all_containers: bool) -> ToolResult {
    let mut args = vec!["ps", "--format", "{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}"];
    if all_containers {
        args.push("-a");
    }

    match run_docker(&args) {
        Ok(stdout) => {
            let mut output = String::from("ID\t\tName\t\tStatus\t\tImage\n");
            output.push_str(&"=".repeat(80));
            output.push('\n');
            output.push_str(&stdout);
            ToolResult::success(output)
        }
        Err(e) => ToolResult::error(e),
    }
}

/// Список образов
pub async fn docker_images() -> ToolResult {
    match run_docker(&["images", "--format", "{{.Repository}}:{{.Tag}}\t{{.ID}}\t{{.Size}}"]) {
        Ok(stdout) => {
            let mut output = String::from("Image\t\t\tID\t\tSize\n");
            output.push_str(&"=".repeat(80));
            output.push('\n');
            output.push_str(&stdout);
            ToolResult::success(output)
        }
        Err(e) => ToolResult::error(e),
    }
}

/// Запустить контейнер
pub async fn docker_run(
    image: &str,
    name: Option<&str>,
    ports: Option<&str>,
    env: Option<&str>,
    volumes: Option<&str>,
    detach: bool,
    command: Option<&str>,
) -> ToolResult {
    let mut args = vec!["run"];
    if detach {
        args.push("-d");
    }
    if let Some(n) = name {
        args.push("--name");
        args.push(n);
    }
    if let Some(p) = ports {
        // Ожидается формат "8080:80,9090:9090"
        for pair in p.split(',').filter(|s| !s.is_empty()) {
            args.push("-p");
            args.push(pair.trim());
        }
    }
    if let Some(e) = env {
        for pair in e.split(',').filter(|s| !s.is_empty()) {
            args.push("-e");
            args.push(pair.trim());
        }
    }
    if let Some(v) = volumes {
        for pair in v.split(',').filter(|s| !s.is_empty()) {
            args.push("-v");
            args.push(pair.trim());
        }
    }
    args.push(image);
    if let Some(cmd) = command {
        args.extend(cmd.split_whitespace());
    }

    match run_docker(&args) {
        Ok(stdout) => {
            let container_id = stdout.trim();
            let mut output = format!("Container started: {}\n", container_id);
            if let Some(n) = name {
                output.push_str(&format!("Name: {}\n", n));
            }
            output.push_str(&format!("Image: {}\n", image));
            ToolResult::success(output)
        }
        Err(e) => ToolResult::error(e),
    }
}

/// Остановить контейнер
pub async fn docker_stop(container: &str) -> ToolResult {
    match run_docker(&["stop", container]) {
        Ok(_) => ToolResult::success(format!("Container stopped: {}", container)),
        Err(e) => ToolResult::error(e),
    }
}

/// Удалить контейнер
pub async fn docker_rm(container: &str, force: bool) -> ToolResult {
    let mut args = vec!["rm"];
    if force {
        args.push("-f");
    }
    args.push(container);

    match run_docker(&args) {
        Ok(_) => ToolResult::success(format!("Container removed: {}", container)),
        Err(e) => ToolResult::error(e),
    }
}

/// Логи контейнера
pub async fn docker_logs(container: &str, tail: usize) -> ToolResult {
    let tail_str = tail.to_string();
    let args = vec!["logs", "--tail", &tail_str, container];
    match run_docker(&args) {
        Ok(stdout) => {
            let output = format!(
                "=== Logs for {} (last {} lines) ===\n\n{}",
                container, tail, stdout
            );
            ToolResult::success(output)
        }
        Err(e) => ToolResult::error(e),
    }
}

/// Выполнить команду в контейнере
pub async fn docker_exec(container: &str, command: &str) -> ToolResult {
    let args = vec!["exec", container, "sh", "-c", command];
    match run_docker(&args) {
        Ok(stdout) => ToolResult::success(stdout),
        Err(e) => ToolResult::error(e),
    }
}

/// Информация о контейнере
pub async fn docker_inspect(container: &str) -> ToolResult {
    match run_docker(&["inspect", container]) {
        Ok(stdout) => {
            match serde_json::from_str::<Vec<Value>>(&stdout) {
                Ok(data) => {
                    if let Some(info) = data.first() {
                        let mut output = format!("=== Container Info: {} ===\n\n", container);
                        
                        if let Some(id) = info.get("Id").and_then(|v| v.as_str()) {
                            output.push_str(&format!("ID: {}\n", crate::truncate_chars(id, 12)));
                        }
                        if let Some(name) = info.get("Name").and_then(|v| v.as_str()) {
                            output.push_str(&format!("Name: {}\n", name));
                        }
                        if let Some(config) = info.get("Config") {
                            if let Some(image) = config.get("Image").and_then(|v| v.as_str()) {
                                output.push_str(&format!("Image: {}\n", image));
                            }
                        }
                        if let Some(state) = info.get("State") {
                            if let Some(status) = state.get("Status").and_then(|v| v.as_str()) {
                                output.push_str(&format!("Status: {}\n", status));
                            }
                            if let Some(running) = state.get("Running").and_then(|v| v.as_bool()) {
                                output.push_str(&format!("Running: {}\n", running));
                            }
                        }

                        // Ports
                        if let Some(network) = info.get("NetworkSettings") {
                            if let Some(ports) = network.get("Ports").and_then(|v| v.as_object()) {
                                if !ports.is_empty() {
                                    output.push_str("\nPorts:\n");
                                    for (container_port, bindings) in ports {
                                        if let Some(bindings) = bindings.as_array() {
                                            for binding in bindings {
                                                if let Some(host_port) = binding.get("HostPort").and_then(|v| v.as_str()) {
                                                    output.push_str(&format!("  {} -> {}\n", host_port, container_port));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Volumes
                        if let Some(mounts) = info.get("Mounts").and_then(|v| v.as_array()) {
                            if !mounts.is_empty() {
                                output.push_str("\nVolumes:\n");
                                for mount in mounts {
                                    let source = mount.get("Source").and_then(|v| v.as_str()).unwrap_or("?");
                                    let dest = mount.get("Destination").and_then(|v| v.as_str()).unwrap_or("?");
                                    output.push_str(&format!("  {} -> {}\n", source, dest));
                                }
                            }
                        }

                        ToolResult::success(output)
                    } else {
                        ToolResult::error("No container data found".to_string())
                    }
                }
                Err(e) => ToolResult::error(format!("Failed to parse inspect output: {}", e)),
            }
        }
        Err(e) => ToolResult::error(e),
    }
}
