use std::collections::HashMap;
use std::time::Duration;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde_json::Value;

/// Результат HTTP запроса
#[derive(Clone)]
pub struct HttpResult {
    pub success: bool,
    pub status_code: Option<u16>,
    pub body: String,
    pub headers: HashMap<String, String>,
    pub error: Option<String>,
}

impl HttpResult {
    pub fn ok(status_code: u16, body: impl Into<String>, headers: HashMap<String, String>) -> Self {
        Self {
            success: true,
            status_code: Some(status_code),
            body: body.into(),
            headers,
            error: None,
        }
    }
    pub fn err(error: impl Into<String>) -> Self {
        Self {
            success: false,
            status_code: None,
            body: String::new(),
            headers: HashMap::new(),
            error: Some(error.into()),
        }
    }
}

/// HTTP инструменты
pub struct HttpTools {
    client: reqwest::Client,
    timeout: Duration,
    user_agent: String,
    default_headers: HeaderMap,
}

impl HttpTools {
    pub fn new() -> Self {
        let mut default_headers = HeaderMap::new();
        default_headers.insert(
            USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (compatible; HephaestusBot/1.0)"),
        );
        
        Self {
            client: reqwest::Client::builder()
                .default_headers(default_headers.clone())
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            timeout: Duration::from_secs(30),
            user_agent: "Mozilla/5.0 (compatible; HephaestusBot/1.0)".to_string(),
            default_headers,
        }
    }

    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout = Duration::from_secs(timeout_secs);
        self.client = reqwest::Client::builder()
            .default_headers(self.default_headers.clone())
            .timeout(self.timeout)
            .build()
            .unwrap_or_default();
        self
    }

    pub fn with_user_agent(mut self, user_agent: &str) -> Self {
        self.user_agent = user_agent.to_string();
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(user_agent).unwrap_or(HeaderValue::from_static("Mozilla/5.0")),
        );
        self.default_headers = headers;
        self.client = reqwest::Client::builder()
            .default_headers(self.default_headers.clone())
            .timeout(self.timeout)
            .build()
            .unwrap_or_default();
        self
    }

    /// GET запрос
    pub async fn get(&self, url: &str, headers: Option<HashMap<String, String>>) -> HttpResult {
        let mut request = self.client.get(url);
        
        if let Some(headers_map) = headers {
            for (key, value) in headers_map {
                if let Ok(header_name) = HeaderName::from_bytes(key.as_bytes()) {
                    if let Ok(header_value) = HeaderValue::from_str(&value) {
                        request = request.header(header_name, header_value);
                    }
                }
            }
        }

        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let headers_map: HashMap<String, String> = response
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                
                match response.text().await {
                    Ok(body) => HttpResult::ok(status, body, headers_map),
                    Err(e) => HttpResult::err(format!("Ошибка чтения тела: {}", e)),
                }
            }
            Err(e) => HttpResult::err(format!("Ошибка запроса: {}", e)),
        }
    }

    /// POST запрос (JSON)
    pub async fn post_json(
        &self,
        url: &str,
        json: &Value,
        headers: Option<HashMap<String, String>>,
    ) -> HttpResult {
        let mut request = self.client.post(url).json(json);
        
        if let Some(headers_map) = headers {
            for (key, value) in headers_map {
                if let Ok(header_name) = HeaderName::from_bytes(key.as_bytes()) {
                    if let Ok(header_value) = HeaderValue::from_str(&value) {
                        request = request.header(header_name, header_value);
                    }
                }
            }
        }

        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let headers_map: HashMap<String, String> = response
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                
                match response.text().await {
                    Ok(body) => HttpResult::ok(status, body, headers_map),
                    Err(e) => HttpResult::err(format!("Ошибка чтения тела: {}", e)),
                }
            }
            Err(e) => HttpResult::err(format!("Ошибка запроса: {}", e)),
        }
    }

    /// POST запрос (form-urlencoded)
    pub async fn post_form(
        &self,
        url: &str,
        form: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
    ) -> HttpResult {
        let mut request = self.client.post(url).form(form);
        
        if let Some(headers_map) = headers {
            for (key, value) in headers_map {
                if let Ok(header_name) = HeaderName::from_bytes(key.as_bytes()) {
                    if let Ok(header_value) = HeaderValue::from_str(&value) {
                        request = request.header(header_name, header_value);
                    }
                }
            }
        }

        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let headers_map: HashMap<String, String> = response
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                
                match response.text().await {
                    Ok(body) => HttpResult::ok(status, body, headers_map),
                    Err(e) => HttpResult::err(format!("Ошибка чтения тела: {}", e)),
                }
            }
            Err(e) => HttpResult::err(format!("Ошибка запроса: {}", e)),
        }
    }

    /// PUT запрос
    pub async fn put(
        &self,
        url: &str,
        body: Option<&str>,
        headers: Option<HashMap<String, String>>,
    ) -> HttpResult {
        let mut request = self.client.put(url);
        
        if let Some(body_str) = body {
            request = request.body(body_str.to_string());
        }

        if let Some(headers_map) = headers {
            for (key, value) in headers_map {
                if let Ok(header_name) = HeaderName::from_bytes(key.as_bytes()) {
                    if let Ok(header_value) = HeaderValue::from_str(&value) {
                        request = request.header(header_name, header_value);
                    }
                }
            }
        }

        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let headers_map: HashMap<String, String> = response
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                
                match response.text().await {
                    Ok(body) => HttpResult::ok(status, body, headers_map),
                    Err(e) => HttpResult::err(format!("Ошибка чтения тела: {}", e)),
                }
            }
            Err(e) => HttpResult::err(format!("Ошибка запроса: {}", e)),
        }
    }

    /// DELETE запрос
    pub async fn delete(&self, url: &str, headers: Option<HashMap<String, String>>) -> HttpResult {
        let mut request = self.client.delete(url);

        if let Some(headers_map) = headers {
            for (key, value) in headers_map {
                if let Ok(header_name) = HeaderName::from_bytes(key.as_bytes()) {
                    if let Ok(header_value) = HeaderValue::from_str(&value) {
                        request = request.header(header_name, header_value);
                    }
                }
            }
        }

        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let headers_map: HashMap<String, String> = response
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                
                match response.text().await {
                    Ok(body) => HttpResult::ok(status, body, headers_map),
                    Err(e) => HttpResult::err(format!("Ошибка чтения тела: {}", e)),
                }
            }
            Err(e) => HttpResult::err(format!("Ошибка запроса: {}", e)),
        }
    }

    /// HEAD запрос
    pub async fn head(&self, url: &str, headers: Option<HashMap<String, String>>) -> HttpResult {
        let mut request = self.client.head(url);

        if let Some(headers_map) = headers {
            for (key, value) in headers_map {
                if let Ok(header_name) = HeaderName::from_bytes(key.as_bytes()) {
                    if let Ok(header_value) = HeaderValue::from_str(&value) {
                        request = request.header(header_name, header_value);
                    }
                }
            }
        }

        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let headers_map: HashMap<String, String> = response
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                
                HttpResult::ok(status, String::new(), headers_map)
            }
            Err(e) => HttpResult::err(format!("Ошибка запроса: {}", e)),
        }
    }

    /// Скачать файл
    pub async fn download(&self, url: &str, save_path: &str) -> HttpResult {
        let response = match self.client.get(url).send().await {
            Ok(resp) => resp,
            Err(e) => return HttpResult::err(format!("Ошибка запроса: {}", e)),
        };

        let status = response.status().as_u16();
        let headers_map: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => return HttpResult::err(format!("Ошибка чтения тела: {}", e)),
        };

        match std::fs::write(save_path, bytes) {
            Ok(_) => HttpResult::ok(status, format!("Файл сохранён: {}", save_path), headers_map),
            Err(e) => HttpResult::err(format!("Ошибка сохранения файла: {}", e)),
        }
    }

    /// Проверить статус URL
    pub async fn check_status(&self, url: &str) -> (bool, u16) {
        match self.client.head(url).send().await {
            Ok(resp) => (resp.status().is_success(), resp.status().as_u16()),
            Err(_) => (false, 0),
        }
    }
}

impl Default for HttpTools {
    fn default() -> Self {
        Self::new()
    }
}

/// Создать HTTP инструменты
pub fn create_http_tools() -> HttpTools {
    HttpTools::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_tools_new() {
        let tools = HttpTools::new();
        assert_eq!(tools.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_http_tools_with_timeout() {
        let tools = HttpTools::new().with_timeout(60);
        assert_eq!(tools.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_http_tools_with_user_agent() {
        let tools = HttpTools::new().with_user_agent("TestBot/1.0");
        assert_eq!(tools.user_agent, "TestBot/1.0");
    }
}
