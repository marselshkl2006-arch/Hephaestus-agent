"""
Vector Search - векторный поиск для улучшения системы памяти.
Использует embeddings и similarity search для семантического поиска.
"""
from __future__ import annotations

import json
import numpy as np
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path
from typing import Any

from .real_tools import ToolResult

try:
    from sentence_transformers import SentenceTransformer
    HAS_SENTENCE_TRANSFORMERS = True
except ImportError:
    HAS_SENTENCE_TRANSFORMERS = False


@dataclass
class VectorDocument:
    """Документ с векторным представлением."""
    doc_id: str
    content: str
    embedding: list[float]
    metadata: dict[str, Any]
    created_at: str


class VectorStore:
    """Векторное хранилище для семантического поиска."""

    def __init__(self, workspace_root: str = ".", model_name: str = "all-MiniLM-L6-v2"):
        self.workspace_root = Path(workspace_root)
        self.vectors_dir = Path.home() / ".claude_code" / "vectors"
        self.vectors_dir.mkdir(parents=True, exist_ok=True)
        self.vectors_file = self.vectors_dir / "vectors.json"

        self.model_name = model_name
        self.model = None
        self.documents: dict[str, VectorDocument] = {}

        if HAS_SENTENCE_TRANSFORMERS:
            try:
                self.model = SentenceTransformer(model_name)
            except Exception as e:
                print(f"⚠️  Warning: Failed to load model {model_name}: {e}")
        else:
            print("⚠️  Warning: sentence-transformers not installed. Vector search disabled!")
            print("   Install with: pip install sentence-transformers")

        self._load_documents()

    def _load_documents(self) -> None:
        """Загрузить документы из файла."""
        if not self.vectors_file.exists():
            return

        try:
            with open(self.vectors_file, 'r', encoding='utf-8') as f:
                data = json.load(f)

            for doc_id, doc_data in data.items():
                self.documents[doc_id] = VectorDocument(**doc_data)

        except Exception as e:
            print(f"Warning: Failed to load vectors: {e}")

    def _save_documents(self) -> None:
        """Сохранить документы в файл."""
        try:
            data = {doc_id: asdict(doc) for doc_id, doc in self.documents.items()}

            with open(self.vectors_file, 'w', encoding='utf-8') as f:
                json.dump(data, f, indent=2, ensure_ascii=False)

        except Exception as e:
            print(f"Error saving vectors: {e}")

    def _get_embedding(self, text: str) -> list[float] | None:
        """Получить embedding для текста."""
        if not self.model:
            return None

        try:
            embedding = self.model.encode(text)
            return embedding.tolist()
        except Exception as e:
            print(f"Error getting embedding: {e}")
            return None

    def _cosine_similarity(self, vec1: list[float], vec2: list[float]) -> float:
        """Вычислить косинусное сходство между векторами."""
        v1 = np.array(vec1)
        v2 = np.array(vec2)

        dot_product = np.dot(v1, v2)
        norm1 = np.linalg.norm(v1)
        norm2 = np.linalg.norm(v2)

        if norm1 == 0 or norm2 == 0:
            return 0.0

        return float(dot_product / (norm1 * norm2))

    def add_document(
        self,
        doc_id: str,
        content: str,
        metadata: dict[str, Any] | None = None
    ) -> ToolResult:
        """
        Добавить документ в векторное хранилище.

        Args:
            doc_id: ID документа
            content: Содержимое документа
            metadata: Метаданные

        Returns:
            ToolResult
        """
        if not self.model:
            return ToolResult(
                success=False,
                output="",
                error="Vector search not available (model not loaded)"
            )

        # Получаем embedding
        embedding = self._get_embedding(content)
        if not embedding:
            return ToolResult(
                success=False,
                output="",
                error="Failed to generate embedding"
            )

        # Создаем документ
        document = VectorDocument(
            doc_id=doc_id,
            content=content,
            embedding=embedding,
            metadata=metadata or {},
            created_at=datetime.now().isoformat()
        )

        self.documents[doc_id] = document
        self._save_documents()

        return ToolResult(
            success=True,
            output=f"Document added: {doc_id}",
            error=None
        )

    def search(
        self,
        query: str,
        top_k: int = 5,
        threshold: float = 0.0
    ) -> ToolResult:
        """
        Поиск похожих документов.

        Args:
            query: Поисковый запрос
            top_k: Количество результатов
            threshold: Минимальный порог сходства (0-1)

        Returns:
            ToolResult с результатами поиска
        """
        if not self.model:
            return ToolResult(
                success=False,
                output="",
                error="Vector search not available (model not loaded)"
            )

        if not self.documents:
            return ToolResult(
                success=True,
                output="No documents in vector store",
                error=None
            )

        # Получаем embedding запроса
        query_embedding = self._get_embedding(query)
        if not query_embedding:
            return ToolResult(
                success=False,
                output="",
                error="Failed to generate query embedding"
            )

        # Вычисляем сходство со всеми документами
        results = []
        for doc in self.documents.values():
            similarity = self._cosine_similarity(query_embedding, doc.embedding)

            if similarity >= threshold:
                results.append({
                    "doc_id": doc.doc_id,
                    "content": doc.content,
                    "similarity": similarity,
                    "metadata": doc.metadata
                })

        # Сортируем по убыванию сходства
        results.sort(key=lambda x: x["similarity"], reverse=True)
        results = results[:top_k]

        if not results:
            return ToolResult(
                success=True,
                output="No similar documents found",
                error=None
            )

        # Форматируем результаты
        output = f"Found {len(results)} similar documents:\n\n"
        for i, result in enumerate(results, 1):
            output += f"{i}. [{result['doc_id']}] (similarity: {result['similarity']:.3f})\n"
            output += f"   {result['content'][:200]}\n"
            if result['metadata']:
                output += f"   Metadata: {result['metadata']}\n"
            output += "\n"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def delete_document(self, doc_id: str) -> ToolResult:
        """
        Удалить документ из хранилища.

        Args:
            doc_id: ID документа

        Returns:
            ToolResult
        """
        if doc_id not in self.documents:
            return ToolResult(
                success=False,
                output="",
                error=f"Document not found: {doc_id}"
            )

        del self.documents[doc_id]
        self._save_documents()

        return ToolResult(
            success=True,
            output=f"Document deleted: {doc_id}",
            error=None
        )

    def list_documents(self) -> ToolResult:
        """
        Список всех документов.

        Returns:
            ToolResult со списком документов
        """
        if not self.documents:
            return ToolResult(
                success=True,
                output="No documents in vector store",
                error=None
            )

        output = f"Vector Store Documents ({len(self.documents)}):\n\n"
        for doc in self.documents.values():
            output += f"[{doc.doc_id}]\n"
            output += f"  Content: {doc.content[:100]}...\n"
            output += f"  Created: {doc.created_at}\n"
            if doc.metadata:
                output += f"  Metadata: {doc.metadata}\n"
            output += "\n"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def get_document(self, doc_id: str) -> ToolResult:
        """
        Получить документ по ID.

        Args:
            doc_id: ID документа

        Returns:
            ToolResult с документом
        """
        doc = self.documents.get(doc_id)
        if not doc:
            return ToolResult(
                success=False,
                output="",
                error=f"Document not found: {doc_id}"
            )

        output = f"Document: {doc.doc_id}\n"
        output += f"Content: {doc.content}\n"
        output += f"Created: {doc.created_at}\n"
        output += f"Metadata: {doc.metadata}\n"

        return ToolResult(
            success=True,
            output=output,
            error=None
        )

    def update_document(
        self,
        doc_id: str,
        content: str | None = None,
        metadata: dict[str, Any] | None = None
    ) -> ToolResult:
        """
        Обновить документ.

        Args:
            doc_id: ID документа
            content: Новое содержимое (опционально)
            metadata: Новые метаданные (опционально)

        Returns:
            ToolResult
        """
        doc = self.documents.get(doc_id)
        if not doc:
            return ToolResult(
                success=False,
                output="",
                error=f"Document not found: {doc_id}"
            )

        if content:
            # Пересчитываем embedding
            embedding = self._get_embedding(content)
            if not embedding:
                return ToolResult(
                    success=False,
                    output="",
                    error="Failed to generate embedding"
                )

            doc.content = content
            doc.embedding = embedding

        if metadata:
            doc.metadata.update(metadata)

        self._save_documents()

        return ToolResult(
            success=True,
            output=f"Document updated: {doc_id}",
            error=None
        )

    def clear(self) -> ToolResult:
        """
        Очистить все документы.

        Returns:
            ToolResult
        """
        count = len(self.documents)
        self.documents.clear()
        self._save_documents()

        return ToolResult(
            success=True,
            output=f"Cleared {count} documents from vector store",
            error=None
        )


# Пример использования
if __name__ == "__main__":
    store = VectorStore()

    # Добавляем документы
    store.add_document(
        "doc1",
        "Python is a high-level programming language",
        metadata={"category": "programming"}
    )

    store.add_document(
        "doc2",
        "JavaScript is used for web development",
        metadata={"category": "programming"}
    )

    store.add_document(
        "doc3",
        "Machine learning is a subset of artificial intelligence",
        metadata={"category": "ai"}
    )

    # Поиск
    result = store.search("programming languages", top_k=2)
    print(result.output)
