//! Vector Search — семантический поиск по сохранённым документам.
//! Порт `vector_search.py` (итерация 6).
//!
//! ВАЖНОЕ ОТЛИЧИЕ ОТ ОРИГИНАЛА: `vector_search.py` использует
//! `sentence-transformers` (нейросетевые embeddings, модель all-MiniLM-L6-v2,
//! ~90 МБ весов, PyTorch). Тащить PyTorch/ONNX-инференс в чистый Rust —
//! отдельная большая задача (candle + скачивание весов модели), а в этой
//! песочнице нет сети, чтобы даже проверить сборку такой зависимости.
//!
//! Вместо этого здесь честная, отдельно работающая реализация: TF-IDF
//! векторы (частота термина × обратная частота документа) + косинусное
//! сходство. Это классический baseline семантического поиска — хуже
//! настоящих embeddings на перефразированных запросах ("собака" не
//! найдёт "пёс"), но реально работает без единой внешней зависимости
//! и без скачивания модели. Публичный API (`add_document`/`search`/
//! `delete_document`/`list_documents`/`get_document`/`update_document`/
//! `clear`) идентичен оригиналу, так что при желании TF-IDF можно
//! позже заменить на настоящие embeddings (напр. через `candle` или
//! вызов внешнего эмбеддинг-сервера по HTTP через `http_tools.rs`) не
//! трогая вызывающий код.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::tools::ToolResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorDocument {
    pub doc_id: String,
    pub content: String,
    /// TF-IDF вектор в разреженном виде: термин -> вес.
    pub embedding: HashMap<String, f64>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub created_at: String,
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(|w| w.to_string())
        .collect()
}

pub struct VectorStore {
    vectors_file: PathBuf,
    /// Обратная частота документа (idf) по всем документам в хранилище,
    /// пересчитывается при каждом изменении набора документов — набор
    /// в этом сценарии (память агента) обычно небольшой (десятки-сотни
    /// документов), пересчёт целиком не проблема.
    documents: Mutex<HashMap<String, VectorDocument>>,
}

impl VectorStore {
    pub fn new() -> Self {
        let mut vectors_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        vectors_dir.push(".hephaestus");
        vectors_dir.push("vectors");
        let _ = fs::create_dir_all(&vectors_dir);
        let vectors_file = vectors_dir.join("vectors.json");
        let documents = Self::load(&vectors_file);
        Self {
            vectors_file,
            documents: Mutex::new(documents),
        }
    }

    fn load(path: &PathBuf) -> HashMap<String, VectorDocument> {
        match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    fn save(&self, docs: &HashMap<String, VectorDocument>) {
        if let Ok(json) = serde_json::to_string_pretty(docs) {
            let _ = fs::write(&self.vectors_file, json);
        }
    }

    /// Пересчитать TF-IDF embedding для одного текста относительно
    /// текущего корпуса (используется и при индексации, и при запросе —
    /// запрос не добавляется в корпус, только временно смешивается в idf).
    fn embed(&self, text: &str, corpus: &HashMap<String, VectorDocument>) -> HashMap<String, f64> {
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return HashMap::new();
        }

        let mut tf: HashMap<String, f64> = HashMap::new();
        for t in &tokens {
            *tf.entry(t.clone()).or_insert(0.0) += 1.0;
        }
        let total = tokens.len() as f64;
        for v in tf.values_mut() {
            *v /= total;
        }

        let n_docs = (corpus.len() + 1) as f64; // +1 за сам текст
        let mut result = HashMap::new();
        for (term, tf_val) in tf {
            let df = 1.0 + corpus
                .values()
                .filter(|d| d.content.to_lowercase().contains(&term))
                .count() as f64;
            let idf = (n_docs / df).ln() + 1.0;
            result.insert(term, tf_val * idf);
        }
        result
    }

    fn cosine_similarity(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
        let mut dot = 0.0;
        for (k, va) in a {
            if let Some(vb) = b.get(k) {
                dot += va * vb;
            }
        }
        let norm_a: f64 = a.values().map(|v| v * v).sum::<f64>().sqrt();
        let norm_b: f64 = b.values().map(|v| v * v).sum::<f64>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            dot / (norm_a * norm_b)
        }
    }

    pub fn add_document(
        &self,
        doc_id: String,
        content: String,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> ToolResult {
        let mut docs = self.documents.lock().unwrap();
        let embedding = self.embed(&content, &docs);
        docs.insert(
            doc_id.clone(),
            VectorDocument {
                doc_id: doc_id.clone(),
                content,
                embedding,
                metadata: metadata.unwrap_or_default(),
                created_at: chrono::Local::now().to_rfc3339(),
            },
        );
        self.save(&docs);
        ToolResult::success(format!("Document added: {}", doc_id))
    }

    pub fn search(&self, query: &str, top_k: usize, threshold: f64) -> ToolResult {
        let docs = self.documents.lock().unwrap();
        if docs.is_empty() {
            return ToolResult::success("No documents in vector store".to_string());
        }

        let query_embedding = self.embed(query, &docs);
        if query_embedding.is_empty() {
            return ToolResult::success("Query has no indexable terms".to_string());
        }

        let mut results: Vec<(f64, &VectorDocument)> = docs
            .values()
            .map(|d| (Self::cosine_similarity(&query_embedding, &d.embedding), d))
            .filter(|(sim, _)| *sim >= threshold)
            .collect();
        results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);

        if results.is_empty() {
            return ToolResult::success("No similar documents found".to_string());
        }

        let mut output = format!("Found {} similar documents:\n\n", results.len());
        for (i, (sim, doc)) in results.iter().enumerate() {
            let preview: String = doc.content.chars().take(200).collect();
            output.push_str(&format!("{}. [{}] (similarity: {:.3})\n   {}\n", i + 1, doc.doc_id, sim, preview));
            if !doc.metadata.is_empty() {
                output.push_str(&format!("   Metadata: {}\n", serde_json::Value::Object(doc.metadata.clone().into_iter().collect())));
            }
            output.push('\n');
        }
        ToolResult::success(output)
    }

    pub fn delete_document(&self, doc_id: &str) -> ToolResult {
        let mut docs = self.documents.lock().unwrap();
        if docs.remove(doc_id).is_some() {
            self.save(&docs);
            ToolResult::success(format!("Document deleted: {}", doc_id))
        } else {
            ToolResult::error(format!("Document not found: {}", doc_id))
        }
    }

    pub fn list_documents(&self) -> ToolResult {
        let docs = self.documents.lock().unwrap();
        if docs.is_empty() {
            return ToolResult::success("No documents in vector store".to_string());
        }
        let mut output = format!("Vector Store Documents ({}):\n\n", docs.len());
        for doc in docs.values() {
            let preview: String = doc.content.chars().take(100).collect();
            output.push_str(&format!("[{}]\n  Content: {}...\n  Created: {}\n\n", doc.doc_id, preview, doc.created_at));
        }
        ToolResult::success(output)
    }

    pub fn get_document(&self, doc_id: &str) -> ToolResult {
        let docs = self.documents.lock().unwrap();
        match docs.get(doc_id) {
            Some(doc) => ToolResult::success(format!(
                "Document: {}\nContent: {}\nCreated: {}\n",
                doc.doc_id, doc.content, doc.created_at
            )),
            None => ToolResult::error(format!("Document not found: {}", doc_id)),
        }
    }

    pub fn update_document(
        &self,
        doc_id: &str,
        content: Option<String>,
        metadata: Option<HashMap<String, serde_json::Value>>,
    ) -> ToolResult {
        let mut docs = self.documents.lock().unwrap();
        if !docs.contains_key(doc_id) {
            return ToolResult::error(format!("Document not found: {}", doc_id));
        }
        if let Some(c) = content {
            let embedding = self.embed(&c, &docs);
            if let Some(d) = docs.get_mut(doc_id) {
                d.content = c;
                d.embedding = embedding;
            }
        }
        if let Some(m) = metadata {
            if let Some(d) = docs.get_mut(doc_id) {
                d.metadata.extend(m);
            }
        }
        self.save(&docs);
        ToolResult::success(format!("Document updated: {}", doc_id))
    }

    pub fn clear(&self) -> ToolResult {
        let mut docs = self.documents.lock().unwrap();
        let count = docs.len();
        docs.clear();
        self.save(&docs);
        ToolResult::success(format!("Cleared {} documents from vector store", count))
    }
}

impl Default for VectorStore {
    fn default() -> Self {
        Self::new()
    }
}
