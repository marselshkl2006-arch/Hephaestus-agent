"""
Skill Learner — автоматическое обучение из успешных Goal выполнений.
После каждого успешного /goal сохраняет план как переиспользуемый skill.
"""
from __future__ import annotations
import json
from datetime import datetime
from pathlib import Path
from .real_tools import ToolResult

SKILLS_LEARNED_FILE = Path.home() / ".hephaestus" / "learned_skills.json"


class SkillLearner:
    def __init__(self):
        SKILLS_LEARNED_FILE.parent.mkdir(parents=True, exist_ok=True)
        self.skills = self._load()

    def _load(self) -> dict:
        if SKILLS_LEARNED_FILE.exists():
            try:
                return json.loads(SKILLS_LEARNED_FILE.read_text(encoding="utf-8"))
            except Exception:
                pass
        return {}

    def _save(self):
        SKILLS_LEARNED_FILE.write_text(
            json.dumps(self.skills, ensure_ascii=False, indent=2), encoding="utf-8"
        )

    def learn_from_goal(self, goal: str, steps: list[dict], success_rate: float) -> str:
        """Сохранить успешный план как skill."""
        if success_rate < 0.7:
            return f"Слишком много ошибок ({success_rate:.0%}), skill не сохранён"

        # Ключ = первые слова цели
        key = "_".join(goal.lower().split()[:4]).replace("/", "").replace("\\", "")
        skill_id = f"learned_{key}_{datetime.now().strftime('%m%d')}"

        self.skills[skill_id] = {
            "goal": goal,
            "steps": steps,
            "success_rate": success_rate,
            "created": datetime.now().isoformat(),
            "uses": 0,
        }
        self._save()
        return skill_id

    def find_similar(self, query: str, top_k: int = 3) -> list[dict]:
        """Найти похожие learned skills для текущей задачи."""
        q = set(query.lower().split())
        scored = []
        for skill_id, skill in self.skills.items():
            goal_words = set(skill["goal"].lower().split())
            score = len(q & goal_words) / max(len(q | goal_words), 1)
            if score > 0.2:
                scored.append((score, skill_id, skill))
        scored.sort(reverse=True)
        return [{"id": sid, **sk} for _, sid, sk in scored[:top_k]]

    def list_skills(self) -> ToolResult:
        if not self.skills:
            return ToolResult(success=True, output="📚 Нет изученных навыков пока. Используй /goal для обучения!")
        lines = [f"📚 Изученные навыки ({len(self.skills)}):\n"]
        for sid, sk in self.skills.items():
            lines.append(f"  🧠 {sid}")
            lines.append(f"     Цель: {sk['goal'][:60]}")
            lines.append(f"     Шагов: {len(sk['steps'])} | Успех: {sk['success_rate']:.0%}")
        return ToolResult(success=True, output="\n".join(lines))
