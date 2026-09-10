use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRisk {
    Safe,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone)]
pub struct SecurityCheck {
    pub allowed: bool,
    pub risk: ActionRisk,
    pub warning: Option<String>,
    pub reason: Option<String>,
}

impl SecurityCheck {
    pub fn allowed(risk: ActionRisk) -> Self {
        Self { allowed: true, risk, warning: None, reason: None }
    }

    pub fn with_warning(mut self, warning: &str) -> Self {
        self.warning = Some(warning.to_string());
        self
    }

    pub fn with_reason(mut self, reason: &str) -> Self {
        self.reason = Some(reason.to_string());
        self
    }
}

pub struct SecurityValidator {
    system_paths: Vec<String>,
    destructive_patterns: Vec<Regex>,
    dangerous_patterns: Vec<Regex>,
    hard_block_patterns: Vec<(Regex, String)>,
    safe_commands: Vec<String>,
    sensitive_patterns: Vec<Regex>,
}

impl SecurityValidator {
    pub fn new() -> Self {
        let windows = cfg!(windows);
        let system_paths: Vec<String> = if windows {
            vec![
                "c:\\windows\\".to_string(), "c:\\program files\\".to_string(),
                "c:\\program files (x86)\\".to_string(), "c:\\boot\\".to_string(),
            ]
        } else {
            vec![
                "/etc/".to_string(), "/sys/".to_string(), "/proc/".to_string(),
                "/dev/".to_string(), "/boot/".to_string(), "/bin/".to_string(),
                "/sbin/".to_string(), "/usr/bin/".to_string(), "/usr/sbin/".to_string(),
            ]
        };

        // Деструктивные паттерны: POSIX и Windows наборы объединены —
        // валидатор один, а команды могут прийти в любой оболочке
        // (WSL-пути, вызов rm из git-bash и т.п.).
        let mut destructive_patterns: Vec<Regex> = vec![
            r"\brm\s+-rf\b",
            r"\bdd\b.*if=",
            r"\bmkfs\b",
            r"\bformat\b",
            r"\bfdisk\b",
            r"\bparted\b",
            r":\(\)\{.*\};\s*:",
        ].into_iter().map(|p| Regex::new(p).unwrap()).collect();
        // WINDOWS: PowerShell/cmd деструктив.
        destructive_patterns.extend([
            r"(?i)Remove-Item\s+[^;\n]*-Recurse[^;\n]*-Force",
            r"(?i)rd\s+/s\s+/q",
            r"(?i)del\s+/s\s+/q",
            r"(?i)reg\s+(delete|add)\s+HKLM",
            r"(?i)bcdedit\s+/set",
            r"(?i)vssadmin\s+delete\s+shadows",
            r"(?i)shutdown\s+/[rts]",
        ].into_iter().map(|p| Regex::new(p).unwrap()));

        let mut dangerous_patterns: Vec<Regex> = vec![
            // ИСПРАВЛЕНО (жалоба "часто ERROR Bash даже на обычных
            // командах"): было `\brm\b` и `\bmv\b.*\s+/` — ловили
            // АБСОЛЮТНО любое использование `rm`/`mv`, даже безобидное
            // `rm file.txt` или `mv a.txt b/c.txt` (у mv практически
            // любой путь назначения содержит "/", так что паттерн
            // срабатывал почти всегда). Это не агент ошибался — это
            // код требовал `force: true` на самые обычные операции,
            // которые агент разумно не помечал как опасные. Настоящая
            // опасность `rm` (рекурсивно/принудительно) уже отдельно
            // ловится ниже в destructive_patterns (`rm -rf` → Critical)
            // и в hard_block_patterns (rm -rf на корень/домашнюю папку
            // → безусловный блок) — здесь дублирование только вредило.
            r"\brm\s+.*-[a-z]*r",   // rm с флагом -r/-rf в любом порядке флагов, но не 'rm -rf /' (уже Critical) — Medium-риск для остальных рекурсивных rm
            r"\bmv\b.*\s+(/etc|/sys|/proc|/dev|/boot|/bin|/sbin|/usr/bin|/usr/sbin)(/|\s|$)", // перемещение В системный путь, а не любой путь со слэшем
            r"\bchmod\b.*777",
            r"\bchown\b.*root",
            r"\bsudo\b",
            r"\bsu\b",
        ].into_iter().map(|p| Regex::new(p).unwrap()).collect();
        // WINDOWS: потенциально системные операции.
        dangerous_patterns.extend([
            r"(?i)Set-ExecutionPolicy",
            r"(?i)Remove-Item\s+[^;\n]*(C:\\Windows|C:\\Program Files)",
            r"(?i)net\s+user\s+\S+\s+/delete",
            r"(?i)taskkill\s+/f\s+/im\s+(explorer|csrss|wininit|svchost)",
        ].into_iter().map(|p| Regex::new(p).unwrap()));

        let mut hard_block_patterns: Vec<(Regex, String)> = vec![
            (r"\brm\s+.*-[a-z]*r[a-z]*f[a-z]*\s+/(\s|$)", "rm -rf на корень файловой системы"),
            (r"\brm\s+.*-[a-z]*r[a-z]*f[a-z]*\s+/\*", "rm -rf на всё содержимое корня"),
            (r"\brm\s+.*-[a-z]*r[a-z]*f[a-z]*\s+(~|\$HOME)(\s|/|$)", "rm -rf на домашнюю папку целиком"),
            (r"--no-preserve-root", "обход защиты rm от удаления корня"),
            (r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:", "fork bomb"),
            (r"\bdd\b[^\n|]*\bof=/dev/(sd|nvme|hd|vd|xvd)", "прямая запись поверх диска целиком"),
            (r"\bmkfs(\.\w+)?\b", "форматирование раздела/диска"),
            (r"\bwipefs\b", "стирание разметки диска"),
            (r">\s*/dev/(sd|nvme|hd|vd|xvd)[a-z0-9]*(\s|$)", "запись напрямую в блочное устройство"),
            (r"\bchmod\s+-R\s+777\s+/(\s|$)", "рекурсивный chmod 777 на корень"),
        ].into_iter().map(|(p, r)| (Regex::new(p).unwrap(), r.to_string())).collect();
        // WINDOWS: безусловные блоки.
        hard_block_patterns.extend([
            (r"(?i)Remove-Item\s+[^;\n]*-Recurse[^;\n]*-Force[^;\n]*\s+C:\\(\s|$)", "Remove-Item -Recurse -Force на корень диска C:\\"),
            (r"(?i)rd\s+/s\s+/q\s+(c:\\|c:$)", "rd /s /q на корень диска"),
            (r"(?i)diskpart", "diskpart — прямая работа с разделами диска"),
            (r"(?i)format\s+[a-z]:\s", "форматирование диска"),
            (r"(?i)cipher\s+/w", "cipher /w — затирание свободного места необратимо"),
        ].into_iter().map(|(p, r)| (Regex::new(p).unwrap(), r.to_string())));

        let safe_commands: Vec<String> = if windows {
            vec![
                "Get-ChildItem", "Get-Content", "Get-Location", "Write-Output", "Get-Date",
                "Select-String", "Get-Process", "dir", "type", "echo", "cls",
                "ls", "cat", "grep", "find", "pwd", "date", "git", "cargo",
            ].into_iter().map(String::from).collect()
        } else {
            vec![
                "ls", "cat", "head", "tail", "grep", "find", "pwd", "echo", "date",
                "git", "cargo",
            ].into_iter().map(String::from).collect()
        };

        let sensitive_patterns: Vec<Regex> = vec![
            r"/\.ssh/", r"/\.aws/", r"/\.env$",
            r"password", r"secret", r"token", r"credentials",
            r"id_rsa", r"id_ed25519",
            // WINDOWS-пути секретов.
            r"(?i)\\.ssh\\", r"(?i)\\.aws\\", r"(?i)\\.env$",
        ].into_iter().map(|p| Regex::new(p).unwrap()).collect();

        let _ = windows;
        Self {
            system_paths,
            destructive_patterns,
            dangerous_patterns,
            hard_block_patterns,
            safe_commands,
            sensitive_patterns,
        }
    }

    pub fn check_catastrophic(&self, command: &str) -> Option<String> {
        for (pattern, reason) in &self.hard_block_patterns {
            if pattern.is_match(command) {
                return Some(reason.clone());
            }
        }
        None
    }

    pub fn check_bash_command(&self, command: &str) -> SecurityCheck {
        if let Some(reason) = self.check_catastrophic(command) {
            return SecurityCheck {
                allowed: false,
                risk: ActionRisk::Critical,
                warning: Some(" БЕЗУСЛОВНАЯ БЛОКИРОВКА!".to_string()),
                reason: Some(reason),
            };
        }

        for pattern in &self.destructive_patterns {
            if pattern.is_match(command) {
                return SecurityCheck::allowed(ActionRisk::Critical)
                    .with_warning("⚠️ КРИТИЧЕСКАЯ ОПАСНОСТЬ: Деструктивная команда!")
                    .with_reason(&format!("Команда может уничтожить данные: {}", command));
            }
        }

        for pattern in &self.dangerous_patterns {
            if pattern.is_match(command) {
                return SecurityCheck::allowed(ActionRisk::High)
                    .with_warning("⚠️ ВЫСОКИЙ РИСК: Опасная команда!")
                    .with_reason(&format!("Команда может изменить систему: {}", command));
            }
        }

        let cmd_name = command.trim().split_whitespace().next().unwrap_or("");
        if self.safe_commands.iter().any(|c| c == cmd_name) {
            return SecurityCheck::allowed(ActionRisk::Safe);
        }

        SecurityCheck::allowed(ActionRisk::Low)
    }

    pub fn check_file_path(&self, file_path: &str, action: &str) -> SecurityCheck {
        for sys_path in &self.system_paths {
            if file_path.starts_with(sys_path) {
                return SecurityCheck::allowed(ActionRisk::High)
                    .with_warning(&format!("⚠️ СИСТЕМНЫЙ ПУТЬ: {}", sys_path))
                    .with_reason("Изменение системных файлов может сломать систему");
            }
        }

        for pattern in &self.sensitive_patterns {
            if pattern.is_match(file_path) {
                return SecurityCheck::allowed(ActionRisk::High)
                    .with_warning("⚠️ ЧУВСТВИТЕЛЬНЫЙ ФАЙЛ!")
                    .with_reason(&format!("Файл может содержать секреты: {}", file_path));
            }
        }

        let risk = match action {
            "read" => ActionRisk::Safe,
            "write" => ActionRisk::Low,
            "delete" => ActionRisk::Medium,
            _ => ActionRisk::Low,
        };

        SecurityCheck::allowed(risk)
    }

    pub fn check_file_write(&self, file_path: &str, _content: &str) -> SecurityCheck {
        self.check_file_path(file_path, "write")
    }

    pub fn check_file_read(&self, file_path: &str) -> SecurityCheck {
        self.check_file_path(file_path, "read")
    }

    pub fn check_file_delete(&self, file_path: &str) -> SecurityCheck {
        let mut check = self.check_file_path(file_path, "delete");
        if check.risk == ActionRisk::Safe || check.risk == ActionRisk::Low {
            check.risk = ActionRisk::Medium;
            check.warning = Some("⚠️ Удаление файла".to_string());
        }
        check
    }
}

impl Default for SecurityValidator {
    fn default() -> Self {
        Self::new()
    }
}