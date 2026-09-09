use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Тип кэша
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CacheType {
    /// Оперативная память (LRU)
    Memory,
    /// Дисковое хранилище
    Disk,
    /// Гибридный (сначала память, потом диск)
    Hybrid,
}

/// Политика вытеснения
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EvictionPolicy {
    /// Least Recently Used
    LRU,
    /// First In First Out
    FIFO,
    /// Time To Live (по времени)
    TTL,
}

/// Запись в кэше
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: String,
    pub value: Value,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub last_accessed: u64,
    pub access_count: u64,
}

impl CacheEntry {
    pub fn new(key: &str, value: Value, ttl_secs: Option<u64>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            key: key.to_string(),
            value,
            created_at: now,
            expires_at: ttl_secs.map(|ttl| now + ttl),
            last_accessed: now,
            access_count: 0,
        }
    }

    pub fn is_expired(&self) -> bool {
        if let Some(expires) = self.expires_at {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now > expires
        } else {
            false
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.access_count += 1;
    }
}

/// Конфигурация кэша
#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub cache_type: CacheType,
    pub max_size: usize,
    pub eviction_policy: EvictionPolicy,
    pub default_ttl_secs: Option<u64>,
    pub disk_path: Option<String>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            cache_type: CacheType::Memory,
            max_size: 1000,
            eviction_policy: EvictionPolicy::LRU,
            default_ttl_secs: Some(3600), // 1 час
            disk_path: None,
        }
    }
}

/// Основная структура кэша
pub struct Cache {
    config: CacheConfig,
    memory_store: HashMap<String, CacheEntry>,
    access_order: Vec<String>, // Для LRU
}

impl Cache {
    pub fn new(config: CacheConfig) -> Self {
        Self {
            config,
            memory_store: HashMap::new(),
            access_order: Vec::new(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(CacheConfig::default())
    }

    /// Получить значение из кэша
    pub fn get(&mut self, key: &str) -> Option<Value> {
        if let Some(entry) = self.memory_store.get_mut(key) {
            if entry.is_expired() {
                self.memory_store.remove(key);
                self.access_order.retain(|k| k != key);
                return None;
            }
            entry.touch();
            let value = entry.value.clone();
            self.update_access_order(key);
            return Some(value);
        }
        None
    }

    /// Получить значение с типом
    pub fn get_typed<T: for<'de> Deserialize<'de>>(&mut self, key: &str) -> Option<T> {
        if let Some(value) = self.get(key) {
            serde_json::from_value(value).ok()
        } else {
            None
        }
    }

    /// Положить значение в кэш
    pub fn set(&mut self, key: &str, value: Value, ttl_secs: Option<u64>) {
        let ttl = ttl_secs.or(self.config.default_ttl_secs);
        let entry = CacheEntry::new(key, value, ttl);

        // Если ключ уже существует, обновляем
        if self.memory_store.contains_key(key) {
            self.memory_store.insert(key.to_string(), entry);
            self.update_access_order(key);
            return;
        }

        // Проверяем размер
        if self.memory_store.len() >= self.config.max_size {
            self.evict();
        }

        self.memory_store.insert(key.to_string(), entry);
        self.access_order.push(key.to_string());
    }

    /// Положить значение с типом
    pub fn set_typed<T: Serialize + ?Sized>(&mut self, key: &str, value: &T, ttl_secs: Option<u64>) -> Result<(), serde_json::Error> {
        let json_value = serde_json::to_value(value)?;
        self.set(key, json_value, ttl_secs);
        Ok(())
    }

    /// Обновить порядок доступа (для LRU)
    fn update_access_order(&mut self, key: &str) {
        if let Some(pos) = self.access_order.iter().position(|k| k == key) {
            self.access_order.remove(pos);
        }
        self.access_order.push(key.to_string());
    }

    /// Вытеснить элемент
    fn evict(&mut self) {
        if self.memory_store.is_empty() {
            return;
        }

        match self.config.eviction_policy {
            EvictionPolicy::LRU => {
                if let Some(key) = self.access_order.first().cloned() {
                    self.memory_store.remove(&key);
                    self.access_order.remove(0);
                }
            }
            EvictionPolicy::FIFO => {
                if let Some(key) = self.access_order.first().cloned() {
                    self.memory_store.remove(&key);
                    self.access_order.remove(0);
                }
            }
            EvictionPolicy::TTL => {
                // Удаляем первый просроченный элемент
                let expired: Vec<String> = self.memory_store
                    .iter()
                    .filter(|(_, entry)| entry.is_expired())
                    .map(|(key, _)| key.clone())
                    .collect();

                if !expired.is_empty() {
                    for key in expired {
                        self.memory_store.remove(&key);
                        self.access_order.retain(|k| k != &key);
                    }
                } else if let Some(key) = self.access_order.first().cloned() {
                    // Если нет просроченных, удаляем самый старый
                    self.memory_store.remove(&key);
                    self.access_order.remove(0);
                }
            }
        }
    }

    /// Проверить существование ключа
    pub fn contains(&mut self, key: &str) -> bool {
        if let Some(entry) = self.memory_store.get(key) {
            if entry.is_expired() {
                self.memory_store.remove(key);
                self.access_order.retain(|k| k != key);
                return false;
            }
            return true;
        }
        false
    }

    /// Удалить ключ
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.access_order.retain(|k| k != key);
        self.memory_store.remove(key).map(|e| e.value)
    }

    /// Очистить кэш
    pub fn clear(&mut self) {
        self.memory_store.clear();
        self.access_order.clear();
    }

    /// Получить размер кэша
    pub fn size(&self) -> usize {
        self.memory_store.len()
    }

    /// Получить статистику
    pub fn stats(&self) -> CacheStats {
        let total = self.memory_store.len();
        let expired = self.memory_store.values().filter(|e| e.is_expired()).count();
        let total_access: u64 = self.memory_store.values().map(|e| e.access_count).sum();

        CacheStats {
            total_entries: total,
            expired_entries: expired,
            total_accesses: total_access,
            max_size: self.config.max_size,
        }
    }

    /// Применить TTL ко всем элементам
    pub fn apply_ttl(&mut self) {
        let expired: Vec<String> = self.memory_store
            .iter()
            .filter(|(_, entry)| entry.is_expired())
            .map(|(key, _)| key.clone())
            .collect();

        for key in expired {
            self.memory_store.remove(&key);
            self.access_order.retain(|k| k != &key);
        }
    }

    /// Обновить TTL для ключа
    pub fn update_ttl(&mut self, key: &str, ttl_secs: u64) -> bool {
        if let Some(entry) = self.memory_store.get_mut(key) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            entry.expires_at = Some(now + ttl_secs);
            true
        } else {
            false
        }
    }

    /// Получить все ключи
    pub fn keys(&self) -> Vec<String> {
        self.memory_store.keys().cloned().collect()
    }
}

/// Статистика кэша
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub total_entries: usize,
    pub expired_entries: usize,
    pub total_accesses: u64,
    pub max_size: usize,
}

impl CacheStats {
    pub fn format(&self) -> String {
        let mut output = String::new();
        output.push_str(" Статистика кэша\n");
        output.push_str(&format!("   Записей: {}\n", self.total_entries));
        output.push_str(&format!("   Просрочено: {}\n", self.expired_entries));
        output.push_str(&format!("   Обращений: {}\n", self.total_accesses));
        output.push_str(&format!("   Максимум: {}\n", self.max_size));
        output
    }
}

/// Кэш с поддержкой нескольких типов значений
pub struct TypedCache {
    inner: Cache,
}

impl TypedCache {
    pub fn new(config: CacheConfig) -> Self {
        Self {
            inner: Cache::new(config),
        }
    }

    pub fn get_string(&mut self, key: &str) -> Option<String> {
        self.inner.get_typed(key)
    }

    pub fn get_i64(&mut self, key: &str) -> Option<i64> {
        self.inner.get_typed(key)
    }

    pub fn get_f64(&mut self, key: &str) -> Option<f64> {
        self.inner.get_typed(key)
    }

    pub fn get_bool(&mut self, key: &str) -> Option<bool> {
        self.inner.get_typed(key)
    }

    pub fn get_vec<T: for<'de> Deserialize<'de>>(&mut self, key: &str) -> Option<Vec<T>> {
        self.inner.get_typed(key)
    }

    pub fn get_map<K: for<'de> Deserialize<'de> + std::cmp::Eq + std::hash::Hash, V: for<'de> Deserialize<'de>>(
        &mut self,
        key: &str,
    ) -> Option<HashMap<K, V>> {
        self.inner.get_typed(key)
    }

    pub fn set_string(&mut self, key: &str, value: &str, ttl_secs: Option<u64>) -> Result<(), serde_json::Error> {
        self.inner.set_typed(key, &value.to_string(), ttl_secs)
    }

    pub fn set_i64(&mut self, key: &str, value: i64, ttl_secs: Option<u64>) -> Result<(), serde_json::Error> {
        self.inner.set_typed(key, &value, ttl_secs)
    }

    pub fn set_f64(&mut self, key: &str, value: f64, ttl_secs: Option<u64>) -> Result<(), serde_json::Error> {
        self.inner.set_typed(key, &value, ttl_secs)
    }

    pub fn set_bool(&mut self, key: &str, value: bool, ttl_secs: Option<u64>) -> Result<(), serde_json::Error> {
        self.inner.set_typed(key, &value, ttl_secs)
    }

    pub fn set_vec<T: Serialize>(
        &mut self,
        key: &str,
        value: &[T],
        ttl_secs: Option<u64>,
    ) -> Result<(), serde_json::Error> {
        self.inner.set_typed(key, value, ttl_secs)
    }

    pub fn set_map<K: Serialize, V: Serialize>(
        &mut self,
        key: &str,
        value: &HashMap<K, V>,
        ttl_secs: Option<u64>,
    ) -> Result<(), serde_json::Error> {
        self.inner.set_typed(key, value, ttl_secs)
    }

    pub fn inner(&mut self) -> &mut Cache {
        &mut self.inner
    }
}

/// Создать кэш с настройками по умолчанию
pub fn create_cache(max_size: usize, ttl_secs: Option<u64>) -> Cache {
    let config = CacheConfig {
        max_size,
        default_ttl_secs: ttl_secs,
        ..Default::default()
    };
    Cache::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_set_get() {
        let mut cache = Cache::with_defaults();
        cache.set("key1", Value::String("value1".to_string()), None);
        let result = cache.get("key1");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Value::String("value1".to_string()));
    }

    #[test]
    fn test_cache_expiration() {
        let mut cache = Cache::with_defaults();
        cache.set("key1", Value::String("value1".to_string()), Some(1));
        std::thread::sleep(Duration::from_secs(2));
        let result = cache.get("key1");
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_eviction_lru() {
        let config = CacheConfig {
            max_size: 2,
            eviction_policy: EvictionPolicy::LRU,
            ..Default::default()
        };
        let mut cache = Cache::new(config);
        cache.set("key1", Value::String("value1".to_string()), None);
        cache.set("key2", Value::String("value2".to_string()), None);
        cache.get("key1"); // Доступ к key1
        cache.set("key3", Value::String("value3".to_string()), None);
        assert!(cache.contains("key1"));
        assert!(!cache.contains("key2"));
    }

    #[test]
    fn test_cache_typed() {
        let mut cache = Cache::with_defaults();
        cache.set_typed("number", &42, None).unwrap();
        let result: Option<i64> = cache.get_typed("number");
        assert_eq!(result, Some(42));
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = Cache::with_defaults();
        cache.set("key1", Value::String("value1".to_string()), None);
        cache.set("key2", Value::String("value2".to_string()), None);
        cache.get("key1");
        cache.get("key1");
        let stats = cache.stats();
        assert_eq!(stats.total_entries, 2);
        assert!(stats.total_accesses >= 2);
    }
}
